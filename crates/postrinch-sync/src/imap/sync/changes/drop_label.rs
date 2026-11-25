//! Deleting a label: `DELETE` of its folder, which deletes the mail in it,
//! so the mail is saved first (docs/imap.md, "Changing mail").
//!
//! What is saved is what the server has in the folder, not what the store
//! knows of it: mail a first read or a backfill has not reached, mail
//! whose headers are owed, and mail just arrived are there too. Every
//! message the folder holds (`UID SEARCH UNDELETED`) is copied to the
//! archive, except one known to have a copy in another folder that the
//! server still has ([`super::verify`]). Where the account keeps every
//! message whatever its labels (Gmail's All Mail), nothing needs saving.
//! Mail another client marked `\Deleted` in the folder goes with it, as
//! that client's next expunge would take it.
//!
//! The folder is listed again after the copies, and anything new copied
//! too, until a listing finds nothing new; then it is deleted. Mail that
//! arrives between that listing and the `DELETE` is lost with the folder:
//! IMAP has no way to delete a folder only if nothing came in.

use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, ImapPlace};

use super::super::ImapSync;
use super::apply::{Exec, known};
use super::commands::{CHUNK, shown};
use super::failure::{Failure, Outcome};
use super::labels::{changeable, mailbox};
use super::plan::{Folders, counted};
use super::record;
use super::verify::where_of;
use crate::SyncResult;

/// How many times the folder is listed again for mail that came in while
/// its mail was being saved, before the label is left for later.
const LISTINGS: usize = 5;

impl ImapSync {
    /// Deletes a label's folder. Its mail stays: whatever is only there is
    /// copied to the archive first. Should anything not be saved, the
    /// folder stays and the error says why.
    pub async fn delete_label(&self, account: &EmailAddress, id: &str) -> SyncResult<()> {
        self.on_folders(account, async |conn, folders| {
            let folder = changeable(folders, id)?;
            let mut exec = Exec::new(conn);
            if !folders.keeps_everything() {
                self.save_mail(&mut exec, account, folders, id).await?;
            }
            if let Some(reason) = exec.take_waits() {
                return Err(Failure::Waits(reason));
            }
            if exec.selected() == Some(id) {
                exec.close().await?;
            }
            let what = format!("deleting {}", shown(&folder.name));
            let deleted = exec
                .conn
                .folder_command(&what, &mailbox("DELETE", &folder.name)?)
                .await;
            if let Err(refused) = deleted {
                // Gone already: the label goes too.
                let gone = matches!(refused, Failure::Refused(_))
                    && self.gone(exec.conn, &folder.name).await?;
                if !gone {
                    return Err(refused);
                }
            }
            let (a, label) = (account.clone(), id.to_owned());
            self.blocking(move |s| {
                let places: Vec<ImapPlace> = s.imap_folder_places(&a, &label)?;
                for place in &places {
                    record::remove_copy(s, &a, place)?;
                }
                s.remove_label(&a, &label)?;
                let messages: HashSet<_> = places.into_iter().map(|p| p.message).collect();
                record::settle(s, &a, &messages)
            })
            .await?;
            Ok(())
        })
        .await
    }

    /// Copies to the archive every message the folder `label` holds that
    /// has no copy elsewhere the server still has, listing the folder
    /// again until nothing new is there.
    async fn save_mail(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &mut Folders,
        label: &str,
    ) -> Outcome<()> {
        let name = known(folders, label)?.name.clone();
        let mut seen: HashSet<u32> = HashSet::new();
        for _ in 0..LISTINGS {
            let validity = exec.open(folders, label).await?;
            let Some(listed) = exec.conn.undeleted().await? else {
                return Err(Failure::Declined(format!(
                    "the server would not list the mail in {}, so it cannot be saved \
                     before the folder goes",
                    shown(&name)
                )));
            };
            let new: Vec<u32> = listed.into_iter().filter(|u| !seen.contains(u)).collect();
            if new.is_empty() {
                return Ok(());
            }
            let to_save = self
                .only_here(exec, account, folders, label, validity, &new)
                .await?;
            if !to_save.is_empty() {
                self.save(exec, account, folders, label, validity, &to_save)
                    .await?;
            }
            seen.extend(new);
        }
        Err(Failure::Waits(format!(
            "mail keeps arriving in {}; the label is deleted once it stops",
            shown(&name)
        )))
    }

    /// Of the UIDs `uids` in the folder `label` (at UIDVALIDITY
    /// `validity`), those to save: all but the ones whose message has a
    /// copy in another folder the server still has.
    async fn only_here(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        label: &str,
        validity: u32,
        uids: &[u32],
    ) -> Outcome<Vec<u32>> {
        let (a, l) = (account.clone(), label.to_owned());
        let kept = self
            .blocking(move |s| {
                let mut by_uid = HashMap::new();
                for place in s.imap_folder_places(&a, &l)? {
                    if place.uidvalidity == validity {
                        let copies = s.imap_message_places(&a, &place.message)?;
                        by_uid.insert(place.uid, copies);
                    }
                }
                Ok(by_uid)
            })
            .await?;
        let elsewhere = |uid: &u32| -> Vec<ImapPlace> {
            kept.get(uid)
                .into_iter()
                .flatten()
                .filter(|p| p.folder != label && counted(p, folders))
                .cloned()
                .collect()
        };
        let others: Vec<ImapPlace> = uids.iter().flat_map(elsewhere).collect();
        let there = self.still_there(exec, account, folders, others).await?;
        Ok(uids
            .iter()
            .copied()
            .filter(|uid| !elsewhere(uid).iter().any(|p| there.contains(&where_of(p))))
            .collect())
    }

    /// Copies the UIDs `uids` of the folder `label` to the archive, making
    /// one where there is none. The copies of messages the store knows are
    /// kept as [`Self::transfer`] keeps them; the rest are new mail to
    /// sync, which finds them in the archive.
    async fn save(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &mut Folders,
        label: &str,
        validity: u32,
        uids: &[u32],
    ) -> Outcome<()> {
        if folders.archive().is_none() {
            self.make_archive(exec.conn, account, folders).await?;
        }
        let archive = folders
            .archive()
            .map(|f| (f.label_id.clone(), f.name.clone()))
            .ok_or_else(|| Failure::Refused("the account has no archive folder".to_owned()))?;
        let wanted: HashSet<u32> = uids.iter().copied().collect();
        let (a, l) = (account.clone(), label.to_owned());
        let places: Vec<ImapPlace> = self
            .blocking(move |s| Ok(s.imap_folder_places(&a, &l)?))
            .await?
            .into_iter()
            .filter(|p| p.uidvalidity == validity && wanted.contains(&p.uid))
            .collect();
        if !places.is_empty() {
            self.transfer(
                exec,
                account,
                folders,
                (label, &archive.0),
                places.clone(),
                false,
            )
            .await?;
        }
        let stored: HashSet<u32> = places.iter().map(|p| p.uid).collect();
        let unknown: Vec<u32> = uids
            .iter()
            .copied()
            .filter(|u| !stored.contains(u))
            .collect();
        exec.open(folders, label).await?;
        for chunk in unknown.chunks(CHUNK) {
            exec.conn.transfer(chunk, &archive.1, false).await?;
        }
        Ok(())
    }
}
