//! Carrying out one change's ops ([`super::plan`], gathered by
//! [`super::change`]) folder by folder, many UIDs to a command, in phases:
//! flags, then copies, then moves ([`super::transfer`]), then deletes, so
//! a copy carries the flags just set and is made before its source moves.
//! Deletes go only when nothing before them waited: a copy skipped because
//! its folder changed on the server may be the one the delete counted on.
//! And a message's deletes go only once each copy its change made or
//! moved is known to have been made (`COPYUID`, or found by its
//! `Message-ID`): a `UID MOVE` or `UID COPY` of a UID another client has
//! just expunged answers `OK` and does nothing, and the deletes after it
//! would take the last real copy. Otherwise they wait, and the change,
//! sent again, is planned from what the server then has.
//!
//! Removing copies is [`super::expunge`]'s: `\Deleted` and an expunge.

use std::collections::{BTreeMap, HashSet};

use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};

use super::super::ImapSync;
use super::super::names::Command;
use super::super::wire::Conn;
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
#[derive(Default)]
pub(super) struct Grouped {
    flags: BTreeMap<(String, &'static str, bool), Vec<ImapPlace>>,
    copies: BTreeMap<(String, String), Vec<ImapPlace>>,
    moves: BTreeMap<(String, String), Vec<ImapPlace>>,
    deletes: BTreeMap<String, Vec<ImapPlace>>,
}

impl Grouped {
    pub(super) fn add(&mut self, op: Op) {
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
    /// Carries out the ops, phase by phase.
    pub(super) async fn run(
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
        let mut unconfirmed: HashSet<ProviderMessageId> = HashSet::new();
        let transfers = (grouped.copies.into_iter().map(|t| (t, false)))
            .chain(grouped.moves.into_iter().map(|t| (t, true)));
        for (((from, to), places), moving) in transfers {
            let messages: Vec<ProviderMessageId> =
                places.iter().map(|p| p.message.clone()).collect();
            let landed = self
                .transfer(exec, account, folders, (&from, &to), places, moving)
                .await?;
            unconfirmed.extend(messages.into_iter().filter(|m| !landed.contains(m)));
        }
        if exec.waits.is_some() {
            return Ok(());
        }
        for (label, places) in grouped.deletes {
            let (go, stay): (Vec<ImapPlace>, Vec<ImapPlace>) = places
                .into_iter()
                .partition(|p| !unconfirmed.contains(&p.message));
            if !stay.is_empty() {
                exec.wait_for(
                    "the server did not say a copy of a message was made, so its other \
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
        if !exec.uidplus {
            self.plain_expunge(exec, account, &uids(&places), &name)
                .await?;
        }
        if self.remove(exec, account, &name, &places).await? {
            self.forget_copies(account, &places).await?;
        }
        Ok(())
    }
}
