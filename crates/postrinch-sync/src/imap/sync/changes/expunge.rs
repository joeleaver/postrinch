//! Removing copies: `\Deleted`, then an expunge (docs/imap.md, "Changing
//! mail").
//!
//! With UIDPLUS, `UID EXPUNGE` removes only ours. Without it a plain
//! `EXPUNGE` removes every message marked `\Deleted` in the folder, so it
//! is sent only when the server says the folder's only such messages are
//! the ones being removed (checked before marking them and again before
//! expunging). Other mail marked there may be another client's mid-delete,
//! about to be expunged: the change waits for it, but only until a sync
//! has seen that mail marked. A client that leaves mail marked and never
//! expunges it (Thunderbird's "mark as deleted", say) would otherwise hold
//! the change for ever; once one has, ours are marked and left unexpunged.
//! Postrinch counts a copy marked `\Deleted` as gone, as sync does, so the
//! change is made, and that client's own expunge removes ours with its
//! own. Other clients' mail is never expunged by Postrinch, but for one
//! race (see [`ImapSync::remove`]).

use std::collections::HashSet;

use postrinch_model::{EmailAddress, ImapPlace};

use super::super::ImapSync;
use super::super::places::DELETED;
use super::apply::{Exec, uids};
use super::commands::{CHUNK, shown};
use super::failure::{Failure, Outcome};
use super::record;

/// What a plain `EXPUNGE` in the selected folder may do (no UIDPLUS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Plain {
    /// Only ours are marked: expunge.
    Expunge,
    /// Other mail is marked, and has stayed so since a sync saw it: ours
    /// are marked and left for that mail's client to expunge.
    LeaveMarked,
}

impl ImapSync {
    /// What a plain `EXPUNGE` in the selected folder (`name`) may do when
    /// `ours` are the UIDs to remove: [`Plain`], or, while other mail
    /// marked there was marked since sync last read the folder, the change
    /// waits.
    pub(super) async fn plain_expunge(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        ours: &[u32],
        name: &str,
    ) -> Outcome<Plain> {
        let ours: HashSet<u32> = ours.iter().copied().collect();
        let others: Vec<u32> = exec
            .conn
            .deleted_uids()
            .await?
            .into_iter()
            .filter(|uid| !ours.contains(uid))
            .collect();
        if others.is_empty() {
            return Ok(Plain::Expunge);
        }
        let Some((label, validity)) = exec.selected_at() else {
            return Err(Failure::Declined("no folder is selected".to_owned()));
        };
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
    /// [`Self::plain_expunge`] says to leave them marked, they stay so
    /// (and as kept, marked). Should other mail be marked since the last
    /// check, the mark is taken off again and the change waits.
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
        match self.plain_expunge(exec, account, &uids(places), name).await {
            Ok(Plain::Expunge) => {
                exec.conn.expunge(None).await?;
                Ok(true)
            }
            Ok(Plain::LeaveMarked) => Ok(false),
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
