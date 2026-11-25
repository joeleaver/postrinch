//! Carrying out one outbox entry: each message's change planned
//! ([`super::plan`]), then done folder by folder, many UIDs to a command,
//! in phases: flags, then copies, then moves, then deletes, so a copy
//! carries the flags just set and is made before its source moves.
//!
//! Without `MOVE`, a move is `COPY`, `STORE +FLAGS (\Deleted)` and an
//! expunge. Without UIDPLUS there is no `UID EXPUNGE`, and a plain
//! `EXPUNGE` removes every message marked `\Deleted` in the folder, so it
//! is sent only when the server says the folder's only such messages are
//! the ones being removed (checked before marking them and again before
//! expunging); otherwise the change waits. Without UIDPLUS a server does
//! not say where copies went (`COPYUID`): they are found among the
//! folder's new mail by their `Message-ID`, from the UIDNEXT it had
//! before; any not found are left to sync ([`super::record`]).

use std::collections::{BTreeMap, HashSet};

use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};
use postrinch_store::OutboxEntry;

use super::super::ImapSync;
use super::super::names::Command;
use super::super::places::{DELETED, Folder};
use super::super::wire::Conn;
use super::commands::{CHUNK, CopyUid, shown};
use super::failure::{Failure, Outcome};
use super::plan::{Folders, Op, Planned, plan};
use super::record;

/// The connection as one change uses it: what it offers, which folder
/// is selected, and what was declined along the way.
pub(super) struct Exec<'c> {
    pub conn: &'c mut Conn,
    uidplus: bool,
    has_move: bool,
    /// The selected folder's label and UIDVALIDITY.
    selected: Option<(String, u32)>,
    /// Why part of the change waits, when it does.
    declined: Option<String>,
}

impl<'c> Exec<'c> {
    pub(super) fn new(conn: &'c mut Conn) -> Self {
        let uidplus = conn.offers("UIDPLUS");
        let has_move = conn.offers("MOVE");
        Self {
            conn,
            uidplus,
            has_move,
            selected: None,
            declined: None,
        }
    }

    /// Selects the folder that is label `label`, unless it is selected,
    /// and returns its UIDVALIDITY.
    async fn open(&mut self, folders: &Folders, label: &str) -> Outcome<u32> {
        if let Some((open, validity)) = &self.selected
            && open == label
        {
            return Ok(*validity);
        }
        let folder = known(folders, label)?;
        self.selected = None;
        let selected = self.conn.select_to_change(&folder.name).await?;
        self.selected = Some((label.to_owned(), selected.uidvalidity));
        Ok(selected.uidvalidity)
    }

    /// The label of the selected folder, if one is.
    pub(super) fn selected(&self) -> Option<&str> {
        self.selected.as_ref().map(|(label, _)| label.as_str())
    }

    /// Leaves the selected folder without expunging (`UNSELECT`), or, where
    /// that is not offered, with `CLOSE`, as before the folder is deleted.
    pub(super) async fn close(&mut self) -> Outcome<()> {
        let verb = if self.conn.offers("UNSELECT") {
            "UNSELECT"
        } else {
            "CLOSE"
        };
        self.selected = None;
        self.conn
            .folder_command("leaving a folder", &Command::plain(verb))
            .await
    }

    /// The copies whose UIDVALIDITY is the folder's: the others' UIDs name
    /// other messages now, so their part waits for sync to read the folder.
    fn current(
        &mut self,
        folders: &Folders,
        validity: u32,
        places: Vec<ImapPlace>,
    ) -> Vec<ImapPlace> {
        let (now, stale): (Vec<ImapPlace>, Vec<ImapPlace>) =
            places.into_iter().partition(|p| p.uidvalidity == validity);
        if let Some(place) = stale.first() {
            let name = folders.get(&place.folder).map_or("", |f| f.name.as_str());
            self.declined.get_or_insert(format!(
                "the folder {} changed on the server since it was read; the change waits for sync",
                shown(name)
            ));
        }
        now
    }

    /// Whether a plain `EXPUNGE` in the selected folder would remove only
    /// `ours` (no UIDPLUS).
    async fn expunge_is_safe(&mut self, ours: &[u32], name: &str) -> Outcome<()> {
        let ours: HashSet<u32> = ours.iter().copied().collect();
        let marked = self.conn.deleted_uids().await?;
        if marked.iter().all(|uid| ours.contains(uid)) {
            return Ok(());
        }
        Err(Failure::Declined(format!(
            "other mail in {} is marked deleted, and the server cannot remove ours alone; \
             the change waits until that mail is gone",
            shown(name)
        )))
    }
}

fn known<'f>(folders: &'f Folders, label: &str) -> Outcome<&'f postrinch_model::ImapFolder> {
    folders
        .get(label)
        .ok_or_else(|| Failure::Refused("a folder this change names is gone".to_owned()))
}

fn uids(places: &[ImapPlace]) -> Vec<u32> {
    places.iter().map(|p| p.uid).collect()
}

/// One entry's ops, grouped by folder.
#[derive(Default)]
struct Grouped {
    flags: BTreeMap<(String, &'static str, bool), Vec<ImapPlace>>,
    copies: BTreeMap<(String, String), Vec<ImapPlace>>,
    moves: BTreeMap<(String, String), Vec<ImapPlace>>,
    deletes: BTreeMap<String, Vec<ImapPlace>>,
}

impl Grouped {
    fn add(&mut self, op: Op) {
        match op {
            Op::Flag { place, flag, add } => self
                .flags
                .entry((place.folder.clone(), flag, add))
                .or_default()
                .push(place),
            Op::Copy { from, to } => self
                .copies
                .entry((from.folder.clone(), to))
                .or_default()
                .push(from),
            Op::Move { from, to } => self
                .moves
                .entry((from.folder.clone(), to))
                .or_default()
                .push(from),
            Op::Delete { place } => self
                .deletes
                .entry(place.folder.clone())
                .or_default()
                .push(place),
        }
    }
}

impl ImapSync {
    /// Makes `entry`'s change on the server, keeping the store's copies in
    /// step, and brings its messages in step with their copies whatever
    /// happened. A message the change cannot be made to does not stop the
    /// others: the entry is refused once the rest is done.
    pub(super) async fn make_change(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        folders: &mut Folders,
        entry: &OutboxEntry,
    ) -> Outcome<()> {
        let (a, ids) = (account.clone(), entry.messages.clone());
        let places = self
            .blocking(move |s| {
                ids.iter()
                    .map(|id| Ok(s.imap_message_places(&a, id)?))
                    .collect::<crate::SyncResult<Vec<Vec<ImapPlace>>>>()
            })
            .await?;
        let mut grouped = Grouped::default();
        let mut refused: Option<String> = None;
        for copies in &places {
            let mut planned = plan(copies, &entry.change, folders);
            if planned == Planned::NeedsArchive {
                self.make_archive(conn, account, folders).await?;
                planned = plan(copies, &entry.change, folders);
            }
            match planned {
                Planned::Ops(ops) => ops.into_iter().for_each(|op| grouped.add(op)),
                Planned::Refused(reason) => {
                    refused.get_or_insert(reason);
                }
                Planned::NeedsArchive => {
                    refused.get_or_insert("the account has no archive folder".to_owned());
                }
            }
        }
        let mut exec = Exec::new(conn);
        let done = self.run(&mut exec, account, folders, grouped).await;
        let declined = exec.declined.take();
        let (a, ids) = (account.clone(), entry.messages.clone());
        self.blocking(move |s| record::settle(s, &a, &ids)).await?;
        done?;
        match (refused, declined) {
            (Some(reason), _) => Err(Failure::Refused(reason)),
            (None, Some(reason)) => Err(Failure::Declined(reason)),
            (None, None) => Ok(()),
        }
    }

    async fn run(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        grouped: Grouped,
    ) -> Outcome<()> {
        for ((label, flag, add), places) in grouped.flags {
            self.flag(exec, account, folders, (&label, flag, add), places)
                .await?;
        }
        for ((from, to), places) in grouped.copies {
            self.transfer(exec, account, folders, (&from, &to), places, false)
                .await?;
        }
        for ((from, to), places) in grouped.moves {
            self.transfer(exec, account, folders, (&from, &to), places, true)
                .await?;
        }
        for (label, places) in grouped.deletes {
            self.delete(exec, account, folders, &label, places).await?;
        }
        Ok(())
    }

    /// Sets (`add`) or clears `flag` on copies in the folder `label`.
    async fn flag(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        (label, flag, add): (&str, &'static str, bool),
        places: Vec<ImapPlace>,
    ) -> Outcome<()> {
        let validity = exec.open(folders, label).await?;
        let places = exec.current(folders, validity, places);
        for chunk in places.chunks(CHUNK) {
            exec.conn.store_flag(&uids(chunk), flag, add).await?;
            let (a, chunk) = (account.clone(), chunk.to_vec());
            self.blocking(move |s| record::set_flag(s, &a, &chunk, flag, add))
                .await?;
        }
        Ok(())
    }

    /// Copies (or moves) copies from the folder `from` into the folder
    /// `to`, as the module says.
    pub(super) async fn transfer(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        (from, to): (&str, &str),
        places: Vec<ImapPlace>,
        moving: bool,
    ) -> Outcome<()> {
        let dest = known(folders, to)?.name.clone();
        let source = known(folders, from)?.name.clone();
        let before = match exec.uidplus {
            true => None,
            false => exec.conn.uid_next(&dest).await?,
        };
        let validity = exec.open(folders, from).await?;
        let places = exec.current(folders, validity, places);
        if places.is_empty() {
            return Ok(());
        }
        let native = moving && exec.has_move;
        let fallback = moving && !exec.has_move;
        if fallback && !exec.uidplus {
            exec.expunge_is_safe(&uids(&places), &source).await?;
        }
        let mut told = true;
        for chunk in places.chunks(CHUNK) {
            match exec.conn.transfer(&uids(chunk), &dest, native).await? {
                Some(copied) => self.keep_copies(account, to, chunk, copied).await?,
                None => told = false,
            }
        }
        let removed = match fallback {
            true => self.remove(exec, account, &source, &places).await,
            false => Ok(()),
        };
        if native || (fallback && removed.is_ok()) {
            self.forget_copies(account, &places).await?;
        }
        let found = match (told, before) {
            (false, Some(before)) => {
                self.find_copies(exec, account, folders, to, before, &places)
                    .await
            }
            _ => Ok(()),
        };
        removed.and(found)
    }

    /// Keeps the copies `COPYUID` says were made of `chunk` in `to`.
    async fn keep_copies(
        &self,
        account: &EmailAddress,
        to: &str,
        chunk: &[ImapPlace],
        copied: CopyUid,
    ) -> Outcome<()> {
        let (a, to, chunk) = (account.clone(), to.to_owned(), chunk.to_vec());
        self.blocking(move |s| {
            for (source, uid) in copied.pairs {
                if let Some(place) = chunk.iter().find(|p| p.uid == source) {
                    record::add_copy(s, &a, place, &to, copied.uidvalidity, uid)?;
                }
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Finds the copies just made in `to` among its mail from the UIDNEXT
    /// it had before (`(uidvalidity, uidnext)`), by `Message-ID`.
    async fn find_copies(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        to: &str,
        (uidvalidity, from): (u32, u32),
        places: &[ImapPlace],
    ) -> Outcome<()> {
        let validity = exec.open(folders, to).await?;
        if validity != uidvalidity {
            return Ok(());
        }
        let reply = exec.conn.copies_from(from).await?;
        let name = known(folders, to)?.name.clone();
        let wanted: HashSet<ProviderMessageId> = places.iter().map(|p| p.message.clone()).collect();
        let (a, to) = (account.clone(), to.to_owned());
        self.blocking(move |s| {
            let folder = Folder {
                label: &to,
                name: &name,
                uidvalidity,
            };
            record::found_copies(s, &a, folder, &reply.messages, &wanted).map(|_| ())
        })
        .await?;
        Ok(())
    }

    /// Deletes copies in one folder.
    async fn delete(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        label: &str,
        places: Vec<ImapPlace>,
    ) -> Outcome<()> {
        let name = known(folders, label)?.name.clone();
        let validity = exec.open(folders, label).await?;
        let places = exec.current(folders, validity, places);
        if places.is_empty() {
            return Ok(());
        }
        if !exec.uidplus {
            exec.expunge_is_safe(&uids(&places), &name).await?;
        }
        self.remove(exec, account, &name, &places).await?;
        self.forget_copies(account, &places).await
    }

    /// Marks copies in the selected folder (`name`) `\Deleted` and
    /// expunges them. Should other mail be marked by the time a plain
    /// `EXPUNGE` would be sent, the mark is taken off again and the
    /// change waits.
    async fn remove(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        name: &str,
        places: &[ImapPlace],
    ) -> Outcome<()> {
        for chunk in places.chunks(CHUNK) {
            exec.conn.store_flag(&uids(chunk), DELETED, true).await?;
            let (a, chunk) = (account.clone(), chunk.to_vec());
            self.blocking(move |s| record::set_flag(s, &a, &chunk, DELETED, true))
                .await?;
        }
        if exec.uidplus {
            for chunk in places.chunks(CHUNK) {
                exec.conn.expunge(Some(&uids(chunk))).await?;
            }
            return Ok(());
        }
        if let Err(unsafe_now) = exec.expunge_is_safe(&uids(places), name).await {
            for chunk in places.chunks(CHUNK) {
                exec.conn.store_flag(&uids(chunk), DELETED, false).await?;
                let (a, chunk) = (account.clone(), chunk.to_vec());
                self.blocking(move |s| record::set_flag(s, &a, &chunk, DELETED, false))
                    .await?;
            }
            return Err(unsafe_now);
        }
        exec.conn.expunge(None).await
    }

    /// Forgets copies the server no longer has.
    async fn forget_copies(&self, account: &EmailAddress, places: &[ImapPlace]) -> Outcome<()> {
        let (a, places) = (account.clone(), places.to_vec());
        self.blocking(move |s| {
            places
                .iter()
                .try_for_each(|p| record::remove_copy(s, &a, p))
        })
        .await?;
        Ok(())
    }
}
