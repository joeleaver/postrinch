//! Carrying out one change's ops ([`super::plan`], gathered by
//! [`super::change`]) folder by folder, many UIDs to a command, in phases:
//! flags, then copies, then moves ([`super::transfer`]), then deletes, so
//! a copy carries the flags just set and is made before its source moves.
//! A move without `MOVE` is a copy and a delete of its source.
//!
//! A message's copies are deleted only when it keeps a copy the server
//! confirmed in this run (docs/imap.md, "Keeping every message safe"):
//! one found by the search before the change ([`super::verify`]) that no
//! op of it moves or deletes, or one a copy or move of it made and the
//! server confirmed. Otherwise the deletes stay and the change waits, or
//! is refused when nothing could ever confirm the copy (a message without
//! a `Message-ID`, on a server without UIDPLUS). A copy the server made
//! none of (its source went between the search and the command) leaves
//! the change waiting too, to be planned again from a copy that is there.
//!
//! Removing copies is [`super::expunge`]'s: `\Deleted` and an expunge.

use std::collections::{BTreeMap, HashSet};

use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};

use super::super::ImapSync;
use super::super::names::Command;
use super::super::wire::Conn;
use super::attempts::Made;
use super::change::Verdict;
use super::commands::{CHUNK, shown};
use super::failure::{Failure, Outcome};
use super::plan::{Folders, Op};
use super::record;

/// The connection as one change uses it: what it offers, which folder
/// is selected, and what waits along the way.
pub(super) struct Exec<'c> {
    pub conn: &'c mut Conn,
    pub uidplus: bool,
    pub has_move: bool,
    /// The selected folder's label and UIDVALIDITY.
    selected: Option<(String, u32)>,
    /// Why part of the change waits, when it does.
    waits: Option<String>,
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
            waits: None,
        }
    }

    /// Why part of the change waits, if it does, now said.
    pub(super) fn take_waits(&mut self) -> Option<String> {
        self.waits.take()
    }

    /// Selects the folder that is label `label`, unless it is selected,
    /// and returns its UIDVALIDITY.
    pub(super) async fn open(&mut self, folders: &Folders, label: &str) -> Outcome<u32> {
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

    /// The label and UIDVALIDITY of the selected folder, if one is.
    pub(super) fn selected_at(&self) -> Option<(String, u32)> {
        self.selected.clone()
    }

    /// Notes why part of the change waits, unless something already did.
    pub(super) fn wait_for(&mut self, reason: String) {
        self.waits.get_or_insert(reason);
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
    pub(super) fn current(
        &mut self,
        folders: &Folders,
        validity: u32,
        places: Vec<ImapPlace>,
    ) -> Vec<ImapPlace> {
        let (now, stale): (Vec<ImapPlace>, Vec<ImapPlace>) =
            places.into_iter().partition(|p| p.uidvalidity == validity);
        if let Some(place) = stale.first() {
            let name = folders.get(&place.folder).map_or("", |f| f.name.as_str());
            self.waits.get_or_insert(format!(
                "the folder {} changed on the server since it was read; the change waits for sync",
                shown(name)
            ));
        }
        now
    }
}

pub(super) fn known<'f>(
    folders: &'f Folders,
    label: &str,
) -> Outcome<&'f postrinch_model::ImapFolder> {
    folders
        .get(label)
        .ok_or_else(|| Failure::Refused("a folder this change names is gone".to_owned()))
}

pub(super) fn uids(places: &[ImapPlace]) -> Vec<u32> {
    places.iter().map(|p| p.uid).collect()
}

/// One entry's ops, grouped by folder.
pub(super) struct Grouped {
    /// Whether the server offers `MOVE`; without it a move is a copy and
    /// a delete.
    has_move: bool,
    flags: BTreeMap<(String, &'static str, bool), Vec<ImapPlace>>,
    copies: BTreeMap<(String, String), Vec<ImapPlace>>,
    moves: BTreeMap<(String, String), Vec<ImapPlace>>,
    deletes: BTreeMap<String, Vec<ImapPlace>>,
}

impl Grouped {
    pub(super) fn new(has_move: bool) -> Self {
        Self {
            has_move,
            flags: BTreeMap::new(),
            copies: BTreeMap::new(),
            moves: BTreeMap::new(),
            deletes: BTreeMap::new(),
        }
    }

    pub(super) fn add(&mut self, op: Op) {
        match op {
            Op::Move { from, to } if !self.has_move => {
                self.add(Op::Copy {
                    from: from.clone(),
                    to,
                });
                self.add(Op::Delete { place: from });
            }
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
    /// Carries out the ops, phase by phase; `anchored`, the messages that
    /// keep a copy the search found and no op moves or deletes.
    pub(super) async fn run(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        grouped: Grouped,
        (anchored, verdict): (&HashSet<ProviderMessageId>, &mut Verdict),
    ) -> Outcome<()> {
        for ((label, flag, add), places) in grouped.flags {
            self.flag(exec, account, folders, (&label, flag, add), places)
                .await?;
        }
        if !exec.uidplus {
            // Nothing is copied where the deletes after it could not go.
            for (label, places) in &grouped.deletes {
                let name = known(folders, label)?.name.clone();
                let validity = exec.open(folders, label).await?;
                let ours: Vec<u32> = places
                    .iter()
                    .filter(|p| p.uidvalidity == validity)
                    .map(|p| p.uid)
                    .collect();
                self.plain_expunge(exec, account, &ours, &name).await?;
            }
        }
        let mut made = Made::default();
        let transfers = (grouped.copies.into_iter().map(|t| (t, false)))
            .chain(grouped.moves.into_iter().map(|t| (t, true)));
        for (((from, to), places), moving) in transfers {
            let one = self
                .transfer(exec, account, folders, (&from, &to), places, moving)
                .await?;
            made.add(one);
        }
        if !made.not_made.is_empty() {
            verdict.waits.get_or_insert(
                "the server made no copy of a message, as the copy it was made from had \
                 just gone; the change is tried again from a copy that is there"
                    .to_owned(),
            );
        }
        for (label, places) in grouped.deletes {
            let (go, stay): (Vec<ImapPlace>, Vec<ImapPlace>) = places
                .into_iter()
                .partition(|p| anchored.contains(&p.message) || made.landed.contains(&p.message));
            if stay.iter().any(|p| made.unknown.contains(&p.message)) {
                verdict.refused.get_or_insert(
                    "the server does not say where it puts a copy, and a message has no \
                     Message-ID to find its copy by, so its other copies stay"
                        .to_owned(),
                );
            } else if !stay.is_empty() {
                verdict.waits.get_or_insert(
                    "the server did not confirm a copy of a message was made, so its other \
                     copies stay; the change is tried again"
                        .to_owned(),
                );
            }
            if !go.is_empty() {
                self.delete(exec, account, folders, &label, go).await?;
            }
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
        if self.remove(exec, account, &name, &places).await? {
            self.forget_copies(account, &places).await?;
        }
        Ok(())
    }
}
