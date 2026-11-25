//! Deleting a label: `DELETE` of its folder, which deletes the mail in it,
//! so the mail is saved first (docs/imap.md, "Changing mail").
//!
//! What is saved is what the server has in the folder, not what the store
//! knows of it: mail a first read or a backfill has not reached, mail
//! whose headers are owed, and mail just arrived are there too. Every
//! message the folder holds (`UID SEARCH UNDELETED`) is copied to the
//! archive, except one known to have a copy in another folder that the
//! server still has ([`super::verify`]); a copy asked for and not seen
//! made keeps nothing. Mail the store does not know is read first (its
//! headers, as sync reads new mail), so every copy made is kept as any
//! change's are, and a deletion tried again copies nothing twice. Mail
//! whose headers the server will not give is copied by its UID, which only
//! `COPYUID` can confirm: without UIDPLUS the deletion is refused, saying
//! why. The folder goes only once every copy is confirmed (docs/imap.md,
//! "Keeping every message safe"). Where the account keeps every message
//! whatever its labels (Gmail's All Mail), nothing needs saving.
//! Mail another client marked `\Deleted` in the folder goes with it, as
//! that client's next expunge would take it.
//!
//! The folder is listed again after the copies, and anything new copied
//! too, until a listing finds nothing new; then it is deleted. Mail that
//! arrives between that listing and the `DELETE` is lost with the folder:
//! IMAP has no way to delete a folder only if nothing came in.

use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};

use super::super::ImapSync;
use super::super::places::Folder;
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
    /// again until nothing new is there. A message whose copy the server
    /// did not make is listed again.
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
            let mut again = Vec::new();
            if !to_save.is_empty() {
                again = self
                    .save(exec, account, folders, (label, validity), &to_save)
                    .await?;
            }
            seen.extend(new);
            again.iter().for_each(|u| {
                seen.remove(u);
            });
        }
        Err(Failure::Waits(format!(
            "mail keeps arriving in {}, or its copies keep going unmade; the label is \
             deleted once it stops",
            shown(&name)
        )))
    }

    /// Of the UIDs `uids` in the folder `label` (at UIDVALIDITY
    /// `validity`), those to save: all but the ones whose message has a
    /// copy in another folder the server is seen to have now, after the
    /// copies earlier tries asked for are settled.
    async fn only_here(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        label: &str,
        validity: u32,
        uids: &[u32],
    ) -> Outcome<Vec<u32>> {
        // Copies an earlier try asked for and did not see made are looked
        // for first: found, they keep their messages as any copy does.
        let (a, l, listed) = (account.clone(), label.to_owned(), uids.to_vec());
        let messages: Vec<ProviderMessageId> = self
            .blocking(move |s| {
                Ok(s.imap_folder_places(&a, &l)?
                    .into_iter()
                    .filter(|p| p.uidvalidity == validity && listed.contains(&p.uid))
                    .map(|p| p.message)
                    .collect())
            })
            .await?;
        self.settle_attempts(exec, account, folders, &messages)
            .await?;
        let (a, l) = (account.clone(), label.to_owned());
        let kept: HashMap<u32, Vec<ImapPlace>> = self
            .blocking(move |s| {
                let mut by_uid = HashMap::new();
                for place in s.imap_folder_places(&a, &l)? {
                    if place.uidvalidity == validity {
                        by_uid.insert(place.uid, s.imap_message_places(&a, &place.message)?);
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
        let there = self
            .still_there(exec, account, folders, others)
            .await?
            .there;
        Ok(uids
            .iter()
            .copied()
            .filter(|uid| !elsewhere(uid).iter().any(|p| there.contains(&where_of(p))))
            .collect())
    }

    /// Copies the UIDs `uids` of the folder `label` (at UIDVALIDITY
    /// `validity`) to the archive, making one where there is none, as
    /// [`Self::transfer`] copies, once the store knows each of them: mail
    /// it does not is read first, and mail whose headers the server will
    /// not give is copied by its UID ([`Self::save_unread`]). Returns the
    /// UIDs whose copy the server did not make, to be listed again. Should
    /// the folder's UIDVALIDITY have changed since it was listed, nothing
    /// is copied and the deletion waits for sync; should a copy be one
    /// nothing can confirm, the deletion is refused.
    async fn save(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &mut Folders,
        (label, validity): (&str, u32),
        uids: &[u32],
    ) -> Outcome<Vec<u32>> {
        if folders.archive().is_none() {
            self.make_archive(exec.conn, account, folders).await?;
        }
        let archive = folders
            .archive()
            .map(|f| f.label_id.clone())
            .ok_or_else(|| Failure::Refused("the account has no archive folder".to_owned()))?;
        let name = known(folders, label)?.name.clone();
        let wanted: HashSet<u32> = uids.iter().copied().collect();
        let mut places = self.kept_at(account, label, validity, &wanted).await?;
        let stored: HashSet<u32> = places.iter().map(|p| p.uid).collect();
        let unknown: Vec<u32> = uids
            .iter()
            .copied()
            .filter(|u| !stored.contains(u))
            .collect();
        let mut again = Vec::new();
        if !unknown.is_empty() {
            if exec.open(folders, label).await? != validity {
                return Err(Failure::Waits(format!(
                    "the folder {} changed on the server since it was listed; the label \
                     is deleted once sync has read it",
                    shown(&name)
                )));
            }
            let read = self
                .read_unknown(exec, account, (label, &name, validity), &unknown)
                .await?;
            let unread: Vec<u32> = unknown.into_iter().filter(|u| !read.contains(u)).collect();
            if !unread.is_empty() {
                again = self
                    .save_unread(exec, folders, (&name, &archive), &unread)
                    .await?;
            }
            places = self.kept_at(account, label, validity, &wanted).await?;
        }
        if places.is_empty() {
            return Ok(again);
        }
        let made = self
            .transfer(
                exec,
                account,
                folders,
                (label, &archive),
                places.clone(),
                false,
            )
            .await?;
        if !made.unknown.is_empty() {
            return Err(Failure::Refused(format!(
                "the server does not say where it puts a copy, and mail in {} has no \
                 Message-ID to find its copy in the archive by, so it cannot be saved for \
                 sure; the folder is kept",
                shown(&name)
            )));
        }
        again.extend(
            places
                .iter()
                .filter(|p| made.not_made.contains(&p.message))
                .map(|p| p.uid),
        );
        Ok(again)
    }

    /// Copies the UIDs `uids` of the selected folder (`name`), whose
    /// headers the server will not give, to the archive (`archive`) by
    /// UID, without waiting for headers that may never come. Only
    /// `COPYUID` can say the copies were made, so without UIDPLUS the
    /// deletion is refused. Returns the UIDs the server did not say it
    /// copied, to be listed again. Sync reads the archive's copies as new
    /// mail.
    async fn save_unread(
        &self,
        exec: &mut Exec<'_>,
        folders: &Folders,
        (name, archive): (&str, &str),
        uids: &[u32],
    ) -> Outcome<Vec<u32>> {
        if !exec.uidplus {
            return Err(Failure::Refused(format!(
                "the server gives no headers for some mail in {}, and does not say where \
                 it puts a copy, so that mail cannot be saved for sure; the folder is kept",
                shown(name)
            )));
        }
        let to = known(folders, archive)?.name.clone();
        let mut copied: HashSet<u32> = HashSet::new();
        for chunk in uids.chunks(CHUNK) {
            if let Some(made) = exec.conn.transfer(chunk, &to, false).await? {
                copied.extend(made.pairs.into_iter().map(|(from, _)| from));
            }
        }
        Ok(uids
            .iter()
            .copied()
            .filter(|u| !copied.contains(u))
            .collect())
    }

    /// The copies kept in the folder `label` at UIDVALIDITY `validity`
    /// whose UIDs are `wanted`.
    async fn kept_at(
        &self,
        account: &EmailAddress,
        label: &str,
        validity: u32,
        wanted: &HashSet<u32>,
    ) -> Outcome<Vec<ImapPlace>> {
        let (a, l) = (account.clone(), label.to_owned());
        Ok(self
            .blocking(move |s| Ok(s.imap_folder_places(&a, &l)?))
            .await?
            .into_iter()
            .filter(|p| p.uidvalidity == validity && wanted.contains(&p.uid))
            .collect())
    }

    /// Reads the headers of the UIDs `uids` of the selected folder
    /// (`(label, name, uidvalidity)`) and keeps them as sync keeps new
    /// mail. Returns the UIDs kept.
    async fn read_unknown(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        (label, name, uidvalidity): (&str, &str, u32),
        uids: &[u32],
    ) -> Outcome<HashSet<u32>> {
        let mut kept = HashSet::new();
        for chunk in uids.chunks(CHUNK) {
            let reply = exec.conn.headers_of(chunk).await?;
            let (a, label, name) = (account.clone(), label.to_owned(), name.to_owned());
            let stored = self
                .blocking(move |s| {
                    let folder = Folder {
                        label: &label,
                        name: &name,
                        uidvalidity,
                    };
                    record::store_read(s, &a, folder, &reply.messages)
                })
                .await?;
            kept.extend(stored);
        }
        Ok(kept)
    }
}
