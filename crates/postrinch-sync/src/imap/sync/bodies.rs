//! Downloading whole messages: backfill, newest first, for the messages
//! stored by their headers, and at once for one the person opens
//! (`BODY.PEEK[]` from any live copy, so reading it does not mark it read).

use std::collections::BTreeMap;

use postrinch_model::{EmailAddress, ImapPlace, Message, ProviderMessageId};
use postrinch_store::Store;

use super::folder_error::{FolderError, FolderResult};
use super::places::first_copy;
use super::wire::{Conn, Fetched, uid_set};
use super::{ImapSync, Lane};
use crate::{Envelope, SyncResult, parse_message};
use crate::{Phase, Progress, ProgressFn, SyncReport};

/// How many messages are downloaded per command, and per hold of the
/// account's sync connection, so a pass waits for at most one batch.
const BODY_BATCH: usize = 20;

/// Where to fetch a message from: of its live copies in a folder whose
/// state says the copy's UIDVALIDITY is current, the one that gave it its
/// headers ([`first_copy`]), so its body and headers are of one copy.
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
        let a = account.clone();
        let ids = self.with_store(move |s| s.undownloaded(&a)).await?;
        let mut report = SyncReport::default();
        let total = ids.len();
        for (i, batch) in ids.chunks(BODY_BATCH).enumerate() {
            progress(Progress::Working {
                phase: Phase::Bodies,
                done: i * BODY_BATCH,
                total,
            });
            let batch = batch.to_vec();
            let result = self
                .over(account, Lane::Sync, async |conn| {
                    self.download(conn, account, batch).await
                })
                .await;
            self.note_sign_in(account, &result).await?;
            report.fetched += result?;
        }
        if total > 0 {
            progress(Progress::Working {
                phase: Phase::Bodies,
                done: total,
                total,
            });
        }
        Ok(report)
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
        let sources = self.blocking(move |s| sources(s, &a, &ids)).await?;
        let mut by_folder: BTreeMap<String, Vec<ImapPlace>> = BTreeMap::new();
        for source in sources {
            by_folder
                .entry(source.folder_name)
                .or_default()
                .push(source.place);
        }
        let mut stored = 0;
        for (name, places) in by_folder {
            match self.download_in(conn, account, &name, places).await {
                Ok(n) => stored += n,
                // The bodies of that folder wait for a later download.
                Err(FolderError::Busy(message)) => tracing::info!(
                    "IMAP: {} is busy for the folder {name:?}: {message}",
                    conn.describe()
                ),
                Err(FolderError::Sync(e)) => return Err(e),
            }
        }
        Ok(stored)
    }

    /// Downloads the bodies of these copies in one folder.
    async fn download_in(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        name: &str,
        places: Vec<ImapPlace>,
    ) -> FolderResult<usize> {
        let Some(selected) = conn.select(name).await? else {
            return Ok(0);
        };
        let current: Vec<ImapPlace> = places
            .into_iter()
            .filter(|p| p.uidvalidity == selected.uidvalidity)
            .collect();
        if current.is_empty() {
            return Ok(0);
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

/// A source for each message that has one.
fn sources(
    store: &Store,
    account: &EmailAddress,
    ids: &[ProviderMessageId],
) -> SyncResult<Vec<Source>> {
    let folders: BTreeMap<String, (String, u32)> = store
        .imap_folders(account)?
        .into_iter()
        .map(|f| (f.label_id, (f.name, f.uidvalidity)))
        .collect();
    let mut out = Vec::new();
    for id in ids {
        let places = store.imap_message_places(account, id)?;
        let current = places.iter().filter(|place| {
            folders
                .get(&place.folder)
                .is_some_and(|(_, uidvalidity)| *uidvalidity == place.uidvalidity)
        });
        let source = first_copy(current).map(|place| Source {
            folder_name: folders[&place.folder].0.clone(),
            place: place.clone(),
        });
        match source {
            Some(source) => out.push(source),
            None => tracing::debug!("IMAP: {account} has no copy of {} to download", id.0),
        }
    }
    Ok(out)
}

/// Stores downloaded messages over what their headers gave. Returns how
/// many were stored.
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
) -> SyncResult<usize> {
    let mut stored = 0;
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
        stored += 1;
    }
    Ok(stored)
}
