//! Removing copies: `\Deleted`, then an expunge (docs/imap.md, "Keeping
//! every message safe").
//!
//! Every copy is noted as marked by Postrinch before it is marked
//! ([`super::super::marks`]), so a mark stranded between the `STORE` and
//! the expunge (a connection lost, a busy answer, a crash) is still known
//! as ours: it never passes for another client's, and a later expunge in
//! its folder takes it ([`ImapSync::settle_marks`], at the next send).
//!
//! With UIDPLUS, `UID EXPUNGE` removes only ours. Without it a plain
//! `EXPUNGE` removes every message marked `\Deleted` in the folder, so it
//! is sent only when the marked mail is all ours: the copies being removed
//! and Postrinch's earlier marks there. That is checked before anything is
//! copied and again once ours are marked. Other mail marked there may be
//! another client's mid-delete, about to be expunged: the change waits for
//! it, but only until a sync has seen that mail marked. A client that
//! leaves mail marked and never expunges it (Thunderbird's "mark as
//! deleted", say) would otherwise hold the change for ever; once one has,
//! ours are marked and left unexpunged, still noted. Postrinch counts a
//! copy marked `\Deleted` as gone, as sync does, so the change is made;
//! the next expunge there once that mail is gone takes ours with it, so
//! they never pile up. Other clients' mail is never expunged by Postrinch,
//! but for one race (see [`ImapSync::remove`]).

use std::collections::{BTreeSet, HashSet};

use postrinch_model::{EmailAddress, ImapPlace};

use super::super::ImapSync;
use super::super::marks;
use super::super::places::DELETED;
use super::super::wire::Conn;
use super::apply::{Exec, uids};
use super::commands::{CHUNK, shown};
use super::failure::{Failure, Outcome};
use super::plan::Folders;
use super::record;
use crate::SyncResult;

/// What a plain `EXPUNGE` in the selected folder may do (no UIDPLUS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Plain {
    /// Only ours are marked: expunge. With the UIDs of Postrinch's
    /// earlier marks there, which go with it.
    Expunge(BTreeSet<u32>),
    /// Other mail is marked, and has stayed so since a sync saw it: ours
    /// are marked and left for that mail's client to expunge.
    LeaveMarked,
}

impl ImapSync {
    /// What a plain `EXPUNGE` in the selected folder (`name`) may do when
    /// `ours` are the UIDs to remove: [`Plain`], or, while other mail
    /// marked there was marked since sync last read the folder, the change
    /// waits. Postrinch's earlier marks there count as ours.
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
            .blocking(move |s| marks::ours(s, &a, (&l, validity), &m))
            .await?;
        let others: Vec<u32> = marked
            .into_iter()
            .filter(|uid| !ours.contains(uid) && !left.contains(uid))
            .collect();
        if others.is_empty() {
            return Ok(Plain::Expunge(left));
        }
        // Every mark of Postrinch's is noted before it is made, so a copy
        // kept as marked among `others` is one sync read marked on the
        // server: another client's.
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
    /// kept, marked, and noted as ours). Should other mail be marked since
    /// the last check, the mark is taken off again and the change waits.
    pub(super) async fn remove(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        name: &str,
        places: &[ImapPlace],
    ) -> Outcome<bool> {
        let Some((label, validity)) = exec.selected_at() else {
            return Err(Failure::Declined("no folder is selected".to_owned()));
        };
        let ours = uids(places);
        self.note_marks(account, (&label, validity), &ours).await?;
        for chunk in places.chunks(CHUNK) {
            exec.conn.store_flag(&uids(chunk), DELETED, true).await?;
            let (a, chunk) = (account.clone(), chunk.to_vec());
            self.blocking(move |s| record::set_flag(s, &a, &chunk, DELETED, true))
                .await?;
        }
        // Again, now that the server has them: a check of the folder by
        // the other connection may have dropped them while in flight.
        self.note_marks(account, (&label, validity), &ours).await?;
        if exec.uidplus {
            for chunk in ours.chunks(CHUNK) {
                exec.conn.expunge(Some(chunk)).await?;
            }
            self.forget_marks(account, (&label, validity), &ours)
                .await?;
            return Ok(true);
        }
        // What is checked here can change before the EXPUNGE arrives: mail
        // another client marks `\Deleted` in between is expunged with ours.
        // IMAP without UIDPLUS has no way to expunge only some messages,
        // so the window can be made short, not closed; that mail was
        // marked to be expunged, and the other client's own expunge would
        // remove it too, unless it took the mark off again first.
        match self.plain_expunge(exec, account, &ours, name).await {
            Ok(Plain::Expunge(left)) => {
                exec.conn.expunge(None).await?;
                self.expunged(account, (&label, validity), left).await?;
                self.forget_marks(account, (&label, validity), &ours)
                    .await?;
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
                self.forget_marks(account, (&label, validity), &ours)
                    .await?;
                Err(not_now)
            }
        }
    }

    /// Expunges Postrinch's marks left in each folder, as the module says:
    /// marks stranded before their expunge, and those left beside another
    /// client's marked mail that has gone since. With UIDPLUS they go by
    /// `UID EXPUNGE`; without it by a plain `EXPUNGE`, only where nothing
    /// else is marked. A folder that cannot be opened or searched now is
    /// left for the next send; only a busy or unreachable server stops it.
    pub(super) async fn settle_marks(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        folders: &Folders,
    ) -> SyncResult<()> {
        let a = account.clone();
        let noted = self.blocking(move |s| marks::folders(s, &a)).await?;
        let mut exec = Exec::new(conn);
        for (label, _) in noted {
            if folders.get(&label).is_none() {
                continue;
            }
            match self
                .settle_marks_in(&mut exec, account, folders, &label)
                .await
            {
                Ok(()) => {}
                Err(Failure::Wait(e)) => return Err(e),
                Err(Failure::Refused(why) | Failure::Declined(why) | Failure::Waits(why)) => {
                    tracing::warn!("IMAP: {account}: marks left in a folder wait: {why}");
                }
            }
        }
        Ok(())
    }

    /// [`Self::settle_marks`] in the folder `label`.
    async fn settle_marks_in(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        label: &str,
    ) -> Outcome<()> {
        let validity = exec.open(folders, label).await?;
        let marked: HashSet<u32> = exec.conn.deleted_uids().await?.into_iter().collect();
        let (a, l, m) = (account.clone(), label.to_owned(), marked.clone());
        let left = self
            .blocking(move |s| marks::ours(s, &a, (&l, validity), &m))
            .await?;
        if left.is_empty() {
            return Ok(());
        }
        if exec.uidplus {
            let left: Vec<u32> = left.iter().copied().collect();
            for chunk in left.chunks(CHUNK) {
                exec.conn.expunge(Some(chunk)).await?;
            }
        } else if marked.iter().all(|uid| left.contains(uid)) {
            exec.conn.expunge(None).await?;
        } else {
            return Ok(());
        }
        self.expunged(account, (label, validity), left).await
    }

    /// Notes `uids` of the folder `label` (at its UIDVALIDITY) as marked
    /// by Postrinch.
    async fn note_marks(
        &self,
        account: &EmailAddress,
        (label, validity): (&str, u32),
        uids: &[u32],
    ) -> Outcome<()> {
        let (a, l, u) = (account.clone(), label.to_owned(), uids.to_vec());
        self.blocking(move |s| marks::note(s, &a, (&l, validity), &u))
            .await?;
        Ok(())
    }

    /// Drops `uids` of the folder `label` from Postrinch's marks.
    async fn forget_marks(
        &self,
        account: &EmailAddress,
        (label, validity): (&str, u32),
        uids: &[u32],
    ) -> Outcome<()> {
        let (a, l, u) = (account.clone(), label.to_owned(), uids.to_vec());
        self.blocking(move |s| marks::forget(s, &a, (&l, validity), &u))
            .await?;
        Ok(())
    }

    /// After an expunge in the folder `label`: Postrinch's marks `gone`
    /// there are forgotten with their copies.
    async fn expunged(
        &self,
        account: &EmailAddress,
        (label, validity): (&str, u32),
        gone: BTreeSet<u32>,
    ) -> Outcome<()> {
        let (a, l) = (account.clone(), label.to_owned());
        self.blocking(move |s| marks::expunged(s, &a, (&l, validity), &gone))
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
