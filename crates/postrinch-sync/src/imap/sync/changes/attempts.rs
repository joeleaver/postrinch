//! Copies asked for and not yet seen made (docs/imap.md, "Keeping every
//! message safe").
//!
//! Before each `UID COPY` or `UID MOVE`, an attempt
//! ([`postrinch_model::ImapCopyAttempt`]) is written for each message,
//! with the destination's UIDVALIDITY and UIDNEXT then. It goes once the
//! copy is confirmed (`COPYUID`, or found by `Message-ID` in a listing of
//! the destination's new mail) or known not to have been made (a listing
//! answered in full without it). An attempt is never a copy: it only stops
//! the copy being made twice. A change sent again settles its messages'
//! attempts first, with a live listing ([`ImapSync::settle_attempts`]), so
//! nothing waits for sync to read past a guess.
//!
//! A message without a `Message-ID` cannot be found by one, so its attempt
//! stays until sync stores its copy there, or the folder goes: it is
//! copied there at most once.

use std::collections::{BTreeMap, HashSet};

use postrinch_model::{EmailAddress, ImapCopyAttempt, ImapPlace, ProviderMessageId};
use postrinch_store::Store;

use super::super::ImapSync;
use super::super::places::{DELETED, Folder};
use super::apply::{Exec, known};
use super::commands::shown;
use super::failure::{Outcome, refusal};
use super::plan::Folders;
use super::record;
use crate::SyncResult;
use crate::imap::named_by_message_id;

/// What became of the copies a transfer was to make, by message.
#[derive(Debug, Default)]
pub(super) struct Made {
    /// Confirmed: `COPYUID`, or found by `Message-ID`.
    pub landed: HashSet<ProviderMessageId>,
    /// Not made: the destination's new mail, listed in full, does not
    /// have it (its source went between the search and the command).
    pub not_made: HashSet<ProviderMessageId>,
    /// Without a `Message-ID` and without `COPYUID`: made or not, nothing
    /// can say.
    pub unknown: HashSet<ProviderMessageId>,
}

impl Made {
    pub(super) fn add(&mut self, other: Made) {
        self.landed.extend(other.landed);
        self.not_made.extend(other.not_made);
        self.unknown.extend(other.unknown);
    }
}

/// The attempts kept for `message`, less those sync or a change has since
/// found the copy of: a live copy in the folder at or past the attempt's
/// UIDNEXT, or at another UIDVALIDITY (the folder read whole again).
pub(super) fn attempts(
    store: &Store,
    account: &EmailAddress,
    message: &ProviderMessageId,
) -> SyncResult<Vec<ImapCopyAttempt>> {
    let places = store.imap_message_places(account, message)?;
    let mut open = Vec::new();
    for attempt in store.imap_copy_attempts(account, message)? {
        let found = places.iter().any(|p| {
            p.folder == attempt.folder
                && !p.flags.iter().any(|f| f.eq_ignore_ascii_case(DELETED))
                && (p.uidvalidity != attempt.uidvalidity || p.uid >= attempt.uidnext)
        });
        match found {
            true => store.remove_imap_copy_attempt(account, &attempt.folder, message)?,
            false => open.push(attempt),
        }
    }
    Ok(open)
}

/// Writes an attempt to copy each of `places`' messages into the folder
/// `to`, whose UIDVALIDITY and UIDNEXT are `before`.
pub(super) fn write(
    store: &Store,
    account: &EmailAddress,
    to: &str,
    (uidvalidity, uidnext): (u32, u32),
    places: &[ImapPlace],
) -> SyncResult<()> {
    for place in places {
        let attempt = ImapCopyAttempt {
            folder: to.to_owned(),
            message: place.message.clone(),
            uidvalidity,
            uidnext,
        };
        store.put_imap_copy_attempt(account, &attempt)?;
    }
    Ok(())
}

impl ImapSync {
    /// Settles the attempts kept for `messages` with a live listing of each
    /// folder they were made in, as the module says: copies found are
    /// recorded, attempts found not made go, and those of a folder whose
    /// UIDVALIDITY changed go too (sync reads it whole). A listing the
    /// server refuses stops the change, to be tried again.
    pub(super) async fn settle_attempts(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        messages: &[ProviderMessageId],
    ) -> Outcome<()> {
        let (a, ids) = (account.clone(), messages.to_vec());
        let open: Vec<ImapCopyAttempt> = self
            .blocking(move |s| {
                let mut open = Vec::new();
                for id in &ids {
                    open.extend(attempts(s, &a, id)?);
                }
                Ok(open)
            })
            .await?;
        let mut by_folder: BTreeMap<String, Vec<ImapCopyAttempt>> = BTreeMap::new();
        for attempt in open.into_iter().filter(|a| named_by_message_id(&a.message)) {
            by_folder
                .entry(attempt.folder.clone())
                .or_default()
                .push(attempt);
        }
        for (label, attempts) in by_folder {
            let (current, stale): (Vec<_>, Vec<_>) = match folders.get(&label) {
                None => (Vec::new(), attempts),
                Some(_) => {
                    let validity = exec.open(folders, &label).await?;
                    attempts
                        .into_iter()
                        .partition(|a| a.uidvalidity == validity)
                }
            };
            if let Some(from) = current.iter().map(|a| a.uidnext).min() {
                let validity = current[0].uidvalidity;
                let messages: Vec<ProviderMessageId> =
                    current.iter().map(|a| a.message.clone()).collect();
                self.look_for(exec, account, folders, &label, (validity, from), &messages)
                    .await?;
            }
            let a = account.clone();
            self.blocking(move |s| {
                stale
                    .iter()
                    .try_for_each(|t| s.remove_imap_copy_attempt(&a, &t.folder, &t.message))
                    .map_err(Into::into)
            })
            .await?;
        }
        Ok(())
    }

    /// Looks for copies of `messages` among the new mail of the folder
    /// `to` from the UIDNEXT it had before the command (`before`, with its
    /// UIDVALIDITY then), by `Message-ID`, recording those found and
    /// forgetting the attempts settled. Messages without a `Message-ID`
    /// are not known either way, and keep their attempts; so do all where
    /// the folder's UIDVALIDITY changed meanwhile, the change then waiting.
    /// A listing refused is recorded as far as it went, then stops the
    /// change, to be tried again.
    pub(super) async fn look_for(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        to: &str,
        (uidvalidity, from): (u32, u32),
        messages: &[ProviderMessageId],
    ) -> Outcome<Made> {
        let mut made = Made::default();
        let (findable, unknown): (Vec<_>, Vec<_>) =
            messages.iter().cloned().partition(named_by_message_id);
        made.unknown.extend(unknown);
        if findable.is_empty() {
            return Ok(made);
        }
        let name = known(folders, to)?.name.clone();
        if exec.open(folders, to).await? != uidvalidity {
            exec.wait_for(format!(
                "the folder {} changed on the server as mail was copied to it; the change \
                 is tried again",
                shown(&name)
            ));
            return Ok(made);
        }
        let reply = exec.conn.copies_from(from).await?;
        let wanted: HashSet<ProviderMessageId> = findable.iter().cloned().collect();
        let new: Vec<_> = reply
            .messages
            .into_iter()
            .filter(|m| m.uid >= from)
            .collect();
        let (a, label, n) = (account.clone(), to.to_owned(), name.clone());
        let complete = reply.refused.is_none();
        let found = self
            .blocking(move |s| {
                let folder = Folder {
                    label: &label,
                    name: &n,
                    uidvalidity,
                };
                let found = record::found_copies(s, &a, folder, &new, &wanted)?;
                let settled = wanted.iter().filter(|m| complete || found.contains(*m));
                for message in settled {
                    s.remove_imap_copy_attempt(&a, &label, message)?;
                }
                Ok(found)
            })
            .await?;
        if let Some(no) = reply.refused {
            let what = format!("listing the mail just copied to {}", shown(&name));
            return Err(refusal(&what, &no));
        }
        made.not_made
            .extend(findable.into_iter().filter(|m| !found.contains(m)));
        made.landed = found;
        Ok(made)
    }
}
