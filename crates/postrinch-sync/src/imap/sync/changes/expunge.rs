//! Removing copies: `\Deleted`, then an expunge (docs/imap.md, "Keeping
//! every message safe").
//!
//! With UIDPLUS, `UID EXPUNGE` removes only ours. Without it a plain
//! `EXPUNGE` removes every message marked `\Deleted` in the folder, so it
//! is sent only when the marked mail is all ours: the copies being removed
//! and those a change left marked there before, which the sync cursor
//! keeps exactly. That is checked before anything is copied and again once
//! ours are marked. Other mail marked there may be another client's
//! mid-delete, about to be expunged: the change waits for it, but only
//! until a sync has seen that mail marked. A client that leaves mail marked
//! and never expunges it (Thunderbird's "mark as deleted", say) would
//! otherwise hold the change for ever; once one has, ours are marked, left
//! unexpunged and noted. Postrinch counts a copy marked `\Deleted` as
//! gone, as sync does, so the change is made; the next delete there once
//! that mail is gone expunges ours with it, so they never pile up. Other
//! clients' mail is never expunged by Postrinch, but for one race (see
//! [`ImapSync::remove`]).

use std::collections::{BTreeSet, HashSet};

use postrinch_model::{EmailAddress, ImapPlace};
use postrinch_store::Store;

use super::super::ImapSync;
use super::super::cursor::{self, Marked};
use super::super::places::DELETED;
use super::apply::{Exec, uids};
use super::commands::{CHUNK, shown};
use super::failure::{Failure, Outcome};
use super::record;
use crate::SyncResult;

/// What a plain `EXPUNGE` in the selected folder may do (no UIDPLUS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Plain {
    /// Only ours are marked: expunge.
    Expunge,
    /// Other mail is marked, and has stayed so since a sync saw it: ours
    /// are marked and left for that mail's client to expunge.
    LeaveMarked,
}

/// The copies of the folder `label`, at UIDVALIDITY `validity`, that
/// changes left marked, still marked now (`marked`): the rest are
/// forgotten, expunged or unmarked by others since.
fn left_marked(
    store: &Store,
    account: &EmailAddress,
    (label, validity): (&str, u32),
    marked: &HashSet<u32>,
) -> SyncResult<BTreeSet<u32>> {
    let kept = cursor::read(store, account)?.marked.remove(label);
    let still: BTreeSet<u32> = kept
        .filter(|m| m.uidvalidity == validity)
        .map(|m| m.uids.into_iter().filter(|u| marked.contains(u)).collect())
        .unwrap_or_default();
    note_left(store, account, label, validity, still.clone())?;
    Ok(still)
}

/// Keeps `uids` as the copies changes left marked in the folder `label`
/// (none: forgets the folder's).
fn note_left(
    store: &Store,
    account: &EmailAddress,
    label: &str,
    uidvalidity: u32,
    uids: BTreeSet<u32>,
) -> SyncResult<()> {
    let now = cursor::read(store, account)?.marked.remove(label);
    let wanted = (!uids.is_empty()).then_some(Marked { uidvalidity, uids });
    if now == wanted {
        return Ok(());
    }
    cursor::update(store, account, |c| match wanted {
        Some(marked) => c.marked.insert(label.to_owned(), marked),
        None => c.marked.remove(label),
    })?;
    Ok(())
}

impl ImapSync {
    /// What a plain `EXPUNGE` in the selected folder (`name`) may do when
    /// `ours` are the UIDs to remove: [`Plain`], or, while other mail
    /// marked there was marked since sync last read the folder, the change
    /// waits. Ours left marked before count as ours.
    pub(super) async fn plain_expunge(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        ours: &[u32],
        name: &str,
    ) -> Outcome<Plain> {
        let Some((label, validity)) = exec.selected_at() else {
            return Err(Failure::Declined("no folder is selected".to_owned()));
        };
        let marked: HashSet<u32> = exec.conn.deleted_uids().await?.into_iter().collect();
        let (a, l, m) = (account.clone(), label.clone(), marked.clone());
        let left = self
            .blocking(move |s| left_marked(s, &a, (&l, validity), &m))
            .await?;
        let others: Vec<u32> = marked
            .into_iter()
            .filter(|uid| !ours.contains(uid) && !left.contains(uid))
            .collect();
        if others.is_empty() {
            return Ok(Plain::Expunge);
        }
        let a = account.clone();
        let seen_marked: HashSet<u32> = self
            .with_store(move |s| {
                Ok(s.imap_folder_places(&a, &label)?
                    .into_iter()
                    .filter(|p| {
                        p.uidvalidity == validity
                            && p.flags.iter().any(|f| f.eq_ignore_ascii_case(DELETED))
                    })
                    .map(|p| p.uid)
                    .collect())
            })
            .await?;
        if others.iter().any(|uid| seen_marked.contains(uid)) {
            return Ok(Plain::LeaveMarked);
        }
        Err(Failure::Waits(format!(
            "other mail in {} was just marked deleted, and the server cannot remove ours \
             alone; the change waits until that mail is gone or sync has seen it",
            shown(name)
        )))
    }

    /// Marks copies in the selected folder (`name`) `\Deleted` and
    /// expunges them, returning whether they were expunged; where
    /// [`Self::plain_expunge`] says to leave them marked, they stay so (as
    /// kept, marked) and are noted as ours. Should other mail be marked
    /// since the last check, the mark is taken off again and the change
    /// waits.
    pub(super) async fn remove(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        name: &str,
        places: &[ImapPlace],
    ) -> Outcome<bool> {
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
            return Ok(true);
        }
        // What is checked here can change before the EXPUNGE arrives: mail
        // another client marks `\Deleted` in between is expunged with ours.
        // IMAP without UIDPLUS has no way to expunge only some messages,
        // so the window can be made short, not closed; that mail was
        // marked to be expunged, and the other client's own expunge would
        // remove it too, unless it took the mark off again first.
        let ours = uids(places);
        let Some((label, validity)) = exec.selected_at() else {
            return Err(Failure::Declined("no folder is selected".to_owned()));
        };
        match self.plain_expunge(exec, account, &ours, name).await {
            Ok(Plain::Expunge) => {
                exec.conn.expunge(None).await?;
                self.forget_left(account, &label, validity).await?;
                Ok(true)
            }
            Ok(Plain::LeaveMarked) => {
                let a = account.clone();
                self.blocking(move |s| {
                    let mut left = cursor::read(s, &a)?
                        .marked
                        .remove(&label)
                        .filter(|m| m.uidvalidity == validity)
                        .map(|m| m.uids)
                        .unwrap_or_default();
                    left.extend(ours);
                    note_left(s, &a, &label, validity, left)
                })
                .await?;
                Ok(false)
            }
            Err(not_now) => {
                for chunk in places.chunks(CHUNK) {
                    exec.conn.store_flag(&uids(chunk), DELETED, false).await?;
                    let (a, chunk) = (account.clone(), chunk.to_vec());
                    self.blocking(move |s| record::set_flag(s, &a, &chunk, DELETED, false))
                        .await?;
                }
                Err(not_now)
            }
        }
    }

    /// After a plain `EXPUNGE` in the folder `label`: the copies changes
    /// left marked there are gone with it, and forgotten.
    async fn forget_left(&self, account: &EmailAddress, label: &str, validity: u32) -> Outcome<()> {
        let (a, label) = (account.clone(), label.to_owned());
        self.blocking(move |s| {
            let Some(left) = cursor::read(s, &a)?.marked.remove(&label) else {
                return Ok(());
            };
            for place in s.imap_folder_places(&a, &label)? {
                if place.uidvalidity == left.uidvalidity && left.uids.contains(&place.uid) {
                    record::remove_copy(s, &a, &place)?;
                }
            }
            note_left(s, &a, &label, validity, BTreeSet::new())
        })
        .await?;
        Ok(())
    }

    /// Forgets copies the server no longer has.
    pub(super) async fn forget_copies(
        &self,
        account: &EmailAddress,
        places: &[ImapPlace],
    ) -> Outcome<()> {
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
