//! Copying and moving copies between folders (`UID COPY`, `UID MOVE`),
//! and keeping where the copies went (docs/imap.md, "Changing mail").
//!
//! Without `MOVE`, a move is `COPY`, then the sources removed as
//! [`super::expunge`] removes copies. The places a command makes are
//! recorded at once: from `COPYUID` (UIDPLUS), or, without UIDPLUS, by
//! finding the copies among the folder's new mail by their `Message-ID`,
//! from the UIDNEXT it had before. One not found so is kept as a copy
//! whose UID is not known ([`postrinch_model::ImapUnplacedCopy`]) until
//! sync finds it, so the change sent again does not copy the message
//! twice; unless the folder has no new mail at all, when the command
//! copied nothing (its sources were gone) and nothing waits for sync.
//!
//! Each transfer says which messages it is known to have copied, so the
//! deletes that count on those copies go only once they are there
//! ([`super::apply`]).

use std::collections::HashSet;

use postrinch_model::{EmailAddress, ImapPlace, ImapUnplacedCopy, ProviderMessageId};

use super::super::ImapSync;
use super::super::cursor;
use super::super::places::Folder;
use super::apply::{Exec, known, uids};
use super::commands::{CHUNK, CopyUid, shown};
use super::failure::{Failure, Outcome};
use super::plan::Folders;
use super::record;

/// The messages a transfer is known to have copied.
pub(super) type Landed = HashSet<ProviderMessageId>;

impl ImapSync {
    /// Copies (or moves) copies from the folder `from` into the folder
    /// `to`, as the module says, and returns the messages known copied.
    pub(super) async fn transfer(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        (from, to): (&str, &str),
        places: Vec<ImapPlace>,
        moving: bool,
    ) -> Outcome<Landed> {
        let dest = known(folders, to)?.name.clone();
        let source = known(folders, from)?.name.clone();
        let before = match exec.uidplus {
            true => None,
            false => match exec.conn.uid_next(&dest).await? {
                Some(before) => Some(before),
                None => {
                    return Err(Failure::Declined(format!(
                        "the server would not say where new mail goes in {}, which it must \
                         for copies to be found without UIDPLUS",
                        shown(&dest)
                    )));
                }
            },
        };
        let validity = exec.open(folders, from).await?;
        let places = exec.current(folders, validity, places);
        let mut landed = Landed::new();
        if places.is_empty() {
            return Ok(landed);
        }
        let native = moving && exec.has_move;
        let fallback = moving && !exec.has_move;
        if fallback && !exec.uidplus {
            self.plain_expunge(exec, account, &uids(&places), &source)
                .await?;
        }
        let mut told = true;
        for chunk in places.chunks(CHUNK) {
            match exec.conn.transfer(&uids(chunk), &dest, native).await? {
                Some(copied) => {
                    landed.extend(self.keep_copies(account, to, chunk, copied).await?);
                }
                None => told = false,
            }
        }
        let removed = match fallback {
            true => self.remove(exec, account, &source, &places).await,
            false => Ok(false),
        };
        if native || matches!(removed, Ok(true)) {
            self.forget_copies(account, &places).await?;
        }
        let found = match (told, before) {
            (false, Some(before)) => {
                self.find_copies(exec, account, folders, to, before, &places)
                    .await
            }
            _ => Ok(Landed::new()),
        };
        removed?;
        landed.extend(found?);
        Ok(landed)
    }

    /// Keeps the copies `COPYUID` says were made of `chunk` in `to`, and
    /// returns their messages.
    async fn keep_copies(
        &self,
        account: &EmailAddress,
        to: &str,
        chunk: &[ImapPlace],
        copied: CopyUid,
    ) -> Outcome<Landed> {
        let (a, to, chunk) = (account.clone(), to.to_owned(), chunk.to_vec());
        let landed = self
            .blocking(move |s| {
                let mut landed = Landed::new();
                for (source, uid) in copied.pairs {
                    if let Some(place) = chunk.iter().find(|p| p.uid == source) {
                        record::add_copy(s, &a, place, &to, copied.uidvalidity, uid)?;
                        landed.insert(place.message.clone());
                    }
                }
                Ok(landed)
            })
            .await?;
        Ok(landed)
    }

    /// Finds the copies just made in `to` among its mail from the UIDNEXT
    /// it had before (`(uidvalidity, uidnext)`), by `Message-ID`, keeping
    /// those not found as copies whose UID is not known, unless the folder
    /// has no new mail at all. Returns the messages found.
    async fn find_copies(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        to: &str,
        (uidvalidity, from): (u32, u32),
        places: &[ImapPlace],
    ) -> Outcome<Landed> {
        let validity = exec.open(folders, to).await?;
        if validity != uidvalidity {
            self.keep_unplaced(account, folders, to, (uidvalidity, from), places)
                .await?;
            return Ok(Landed::new());
        }
        let reply = exec.conn.copies_from(from).await?;
        // The server answered in full and has nothing from that UIDNEXT
        // on: the command copied nothing, as when its sources were gone.
        if reply.refused.is_none() && reply.messages.iter().all(|m| m.uid < from) {
            return Ok(Landed::new());
        }
        let name = known(folders, to)?.name.clone();
        let wanted: HashSet<ProviderMessageId> = places.iter().map(|p| p.message.clone()).collect();
        let (a, label) = (account.clone(), to.to_owned());
        let found = self
            .blocking(move |s| {
                let folder = Folder {
                    label: &label,
                    name: &name,
                    uidvalidity,
                };
                record::found_copies(s, &a, folder, &reply.messages, &wanted)
            })
            .await?;
        let missing: Vec<ImapPlace> = places
            .iter()
            .filter(|p| !found.contains(&p.message))
            .cloned()
            .collect();
        self.keep_unplaced(account, folders, to, (uidvalidity, from), &missing)
            .await?;
        Ok(found)
    }

    /// Keeps the copies of `places` just made in `to` as copies whose UID
    /// is not known, from the UIDVALIDITY and UIDNEXT `to` had before and
    /// the sync pass last run.
    async fn keep_unplaced(
        &self,
        account: &EmailAddress,
        folders: &Folders,
        to: &str,
        (uidvalidity, uidnext): (u32, u32),
        places: &[ImapPlace],
    ) -> Outcome<()> {
        if places.is_empty() {
            return Ok(());
        }
        let read_uidvalidity = known(folders, to)?.uidvalidity;
        let (a, to) = (account.clone(), to.to_owned());
        let messages: Vec<ProviderMessageId> = places.iter().map(|p| p.message.clone()).collect();
        self.blocking(move |s| {
            let pass = cursor::read(s, &a)?.pass;
            messages.iter().try_for_each(|message| {
                let copy = ImapUnplacedCopy {
                    folder: to.clone(),
                    message: message.clone(),
                    uidvalidity,
                    uidnext,
                    read_uidvalidity,
                    pass,
                };
                Ok(s.put_imap_unplaced_copy(&a, &copy)?)
            })
        })
        .await?;
        Ok(())
    }
}
