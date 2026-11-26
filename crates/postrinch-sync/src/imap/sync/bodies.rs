//! Downloading whole messages: backfill, newest first, for the messages
//! stored by their headers, and at once for one the person opens
//! (`BODY.PEEK[]` from any live copy, so reading it does not mark it read).
//!
//! A message is fetched from the copy that gave it its headers when it
//! can be ([`first_copy`](super::places::first_copy)), so its body and
//! headers are of one copy. A copy is fetched only once `SELECT` confirms
//! its own UIDVALIDITY, so copies a search stored in a folder no pass has
//! read yet, or after the folder's UIDVALIDITY changed (search.rs), open
//! too: the folder's state is left as the last pass read it. A message
//! whose copy could not be fetched (gone, busy, another UIDVALIDITY) is
//! tried from its next copy.

use std::collections::{BTreeMap, HashSet};

use postrinch_model::{EmailAddress, ImapPlace, Message, ProviderMessageId};
use postrinch_store::Store;

use super::folder_error::{FolderError, FolderResult};
use super::places::live_copies;
use super::wire::{Conn, Fetched, uid_set};
use super::{ImapSync, Lane};
use crate::giving_way::{Backfilled, Unfinished};
use crate::{Envelope, SyncResult, parse_message};
use crate::{Phase, Progress, ProgressFn, SyncReport};

/// How many messages are downloaded per command, and per hold of the
/// account's sync connection, so a pass waits for at most one batch.
const BODY_BATCH: usize = 20;

/// A copy to fetch a message from.
#[derive(Debug, Clone)]
struct Source {
    folder_name: String,
    place: ImapPlace,
}

impl ImapSync {
    /// Downloads every message the store has only headers for, newest
    /// first, over the account's sync connection, a batch at a time.
    /// Stopping it at any point is safe; the next call carries on from the
    /// newest message still left.
    pub async fn backfill(
        &self,
        account: &EmailAddress,
        progress: ProgressFn,
    ) -> SyncResult<SyncReport> {
        Ok(self
            .backfill_until(account, progress, || false)
            .await
            .map_err(|unfinished| unfinished.error)?
            .report)
    }

    /// As [`Self::backfill`], giving way between batches when
    /// `give_way` says a pass is due: after at least one batch, so each
    /// call gets on however often passes come, and never after the last,
    /// so one that finished says so. A failure says how many messages
    /// were gone through before it.
    pub(crate) async fn backfill_until(
        &self,
        account: &EmailAddress,
        progress: ProgressFn,
        mut give_way: impl FnMut() -> bool + Send,
    ) -> Result<Backfilled, Unfinished> {
        let a = account.clone();
        let ids = self.with_store(move |s| s.undownloaded(&a)).await?;
        let mut out = Backfilled::default();
        let total = ids.len();
        if total == 0 {
            return Ok(out);
        }
        let working = |done| Progress::Working {
            phase: Phase::Bodies,
            done,
            total,
        };
        progress(working(0));
        for batch in ids.chunks(BODY_BATCH) {
            let batch = batch.to_vec();
            let count = batch.len();
            let result = self
                .over(account, Lane::Sync, async |conn| {
                    self.download(conn, account, batch).await
                })
                .await;
            let failed = |error| Unfinished {
                through: out.through,
                error,
            };
            self.note_sign_in(account, &result).await.map_err(failed)?;
            out.report.fetched += result.map_err(failed)?;
            out.through += count;
            progress(working(out.through));
            if out.through < total && give_way() {
                out.gave_way = true;
                break;
            }
        }
        Ok(out)
    }

    /// Downloads these messages now, over the account's reader connection:
    /// the person is waiting for them. Returns how many were stored whole;
    /// one with no copy left on the server is skipped (the next pass
    /// removes it).
    pub async fn download_now(
        &self,
        account: &EmailAddress,
        ids: Vec<ProviderMessageId>,
    ) -> SyncResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let result = self
            .over(account, Lane::Reader, async |conn| {
                self.download(conn, account, ids).await
            })
            .await;
        self.note_sign_in(account, &result).await?;
        result
    }

    async fn download(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        ids: Vec<ProviderMessageId>,
    ) -> SyncResult<usize> {
        let a = account.clone();
        // Each message's copies not tried yet, the best first.
        let mut left = self.blocking(move |s| sources(s, &a, &ids)).await?;
        let mut stored = 0;
        loop {
            let mut by_folder: BTreeMap<String, Vec<ImapPlace>> = BTreeMap::new();
            for copies in &mut left {
                if !copies.is_empty() {
                    let next = copies.remove(0);
                    by_folder
                        .entry(next.folder_name)
                        .or_default()
                        .push(next.place);
                }
            }
            if by_folder.is_empty() {
                return Ok(stored);
            }
            let mut done: HashSet<ProviderMessageId> = HashSet::new();
            for (name, places) in by_folder {
                match self.download_in(conn, account, &name, places).await {
                    Ok(ids) => done.extend(ids),
                    // Its messages are tried from another copy, if they have
                    // one, or wait for a later download.
                    Err(FolderError::Busy(message)) => tracing::info!(
                        "IMAP: {} is busy for the folder {name:?}: {message}",
                        conn.describe()
                    ),
                    Err(FolderError::Sync(e)) => return Err(e),
                }
            }
            stored += done.len();
            left.retain(|copies| {
                copies
                    .first()
                    .is_some_and(|c| !done.contains(&c.place.message))
            });
        }
    }

    /// Downloads the bodies of these copies in one folder: those whose
    /// UIDVALIDITY is the folder's now. Returns the messages stored.
    async fn download_in(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        name: &str,
        places: Vec<ImapPlace>,
    ) -> FolderResult<Vec<ProviderMessageId>> {
        let Some(selected) = conn.select(name).await? else {
            return Ok(Vec::new());
        };
        let current: Vec<ImapPlace> = places
            .into_iter()
            .filter(|p| p.uidvalidity == selected.uidvalidity)
            .collect();
        if current.is_empty() {
            return Ok(Vec::new());
        }
        let uids: Vec<u32> = current.iter().map(|p| p.uid).collect();
        let fetched = conn
            .uid_fetch(&uid_set(&uids), "(UID BODY.PEEK[])", None)
            .await?
            .messages;
        let (a, name) = (account.clone(), name.to_owned());
        Ok(self
            .blocking(move |s| store_bodies(s, &a, &name, &current, fetched))
            .await?)
    }
}

/// Each message's copies to fetch it from, best first: those whose
/// UIDVALIDITY the folder's state has, then those of another (a folder
/// not read yet, or read before its UIDVALIDITY changed), each in the
/// order of [`live_copies`]. A message with none is left out (the next
/// pass removes it).
fn sources(
    store: &Store,
    account: &EmailAddress,
    ids: &[ProviderMessageId],
) -> SyncResult<Vec<Vec<Source>>> {
    let folders: BTreeMap<String, (String, u32)> = store
        .imap_folders(account)?
        .into_iter()
        .map(|f| (f.label_id, (f.name, f.uidvalidity)))
        .collect();
    let mut out = Vec::new();
    for id in ids {
        let places = store.imap_message_places(account, id)?;
        let mut copies: Vec<(bool, Source)> = live_copies(&places)
            .into_iter()
            .filter_map(|place| {
                let (name, uidvalidity) = folders.get(&place.folder)?;
                let confirmed = *uidvalidity != 0 && *uidvalidity == place.uidvalidity;
                let source = Source {
                    folder_name: name.clone(),
                    place: place.clone(),
                };
                Some((confirmed, source))
            })
            .collect();
        // Stable: within each, the order of live_copies.
        copies.sort_by_key(|(confirmed, _)| !confirmed);
        if copies.is_empty() {
            tracing::debug!("IMAP: {account} has no copy of {} to download", id.0);
        } else {
            out.push(copies.into_iter().map(|(_, s)| s).collect());
        }
    }
    Ok(out)
}

/// Stores downloaded messages over what their headers gave. Returns the
/// messages stored.
///
/// A message that cannot be parsed (a zero-byte one, as maildir servers
/// can hold) is kept as downloaded with its raw bytes and what its
/// headers gave, so backfill does not ask for it again on every pass and
/// the person can still see it and save it.
fn store_bodies(
    store: &Store,
    account: &EmailAddress,
    folder_name: &str,
    places: &[ImapPlace],
    fetched: Vec<Fetched>,
) -> SyncResult<Vec<ProviderMessageId>> {
    let mut stored = Vec::new();
    for f in fetched {
        let (Some(place), Some(bytes)) = (places.iter().find(|p| p.uid == f.uid), f.body) else {
            continue;
        };
        // Its labels, flags, thread and time are what sync gave it.
        let Some(kept) = store.message(account, &place.message)? else {
            continue;
        };
        let envelope = Envelope {
            account: account.clone(),
            id: kept.id.clone(),
            thread: kept.thread.clone(),
            received: kept.received,
            labels: kept.labels.clone(),
            flags: kept.flags,
            snippet: None,
        };
        let message = match parse_message(envelope, &bytes) {
            Ok(message) => message,
            Err(e) => {
                tracing::warn!(
                    "IMAP: UID {} in {folder_name:?} of {account} cannot be read ({e}); \
                     it is kept as the server sent it",
                    f.uid
                );
                Message {
                    size: bytes.len() as u64,
                    downloaded: true,
                    ..kept
                }
            }
        };
        store.put_message(&message, Some(&bytes))?;
        stored.push(place.message.clone());
    }
    Ok(stored)
}
