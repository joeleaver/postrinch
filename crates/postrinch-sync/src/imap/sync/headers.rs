//! Fetching copies by their headers, newest first, and what becomes of
//! the ones the server does not answer for ([`super::reads`]).

use std::collections::{BTreeMap, BTreeSet};

use postrinch_model::{EmailAddress, ImapFolder};

use super::ImapSync;
use super::folder_error::FolderResult;
use super::places::{self, Folder, Stored};
use super::reads::SILENT_TRIES;
use super::wire::{Conn, Fetched, uid_set};
use crate::SyncResult;
use crate::google::{Phase, Progress, ProgressFn, SyncReport};

/// How many messages' headers are asked for at once.
const HEADER_BATCH: usize = 100;

impl ImapSync {
    /// Fetches and stores copies by their headers, newest first: each UID
    /// with how many passes it went unanswered in before this one.
    /// Returns the UIDs still unanswered, with their new counts: a UID
    /// that reaches [`SILENT_TRIES`] is stored as a message that cannot
    /// be read instead. A `NO` ends only its batch: what came is stored,
    /// and the rest is unanswered. A server too busy to answer stops the
    /// folder or the pass instead, keeping the batches already stored
    /// and counting nothing against the messages
    /// ([`super::folder_error::FolderError::Busy`],
    /// [`crate::SyncError::Busy`]).
    pub(super) async fn fetch_headers(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        state: &ImapFolder,
        wanted: BTreeMap<u32, u32>,
        progress: &ProgressFn,
        report: &mut SyncReport,
    ) -> FolderResult<BTreeMap<u32, u32>> {
        let uids: Vec<u32> = wanted.keys().rev().copied().collect();
        let total = uids.len();
        let items = if conn.condstore() {
            "(UID FLAGS MODSEQ RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])"
        } else {
            "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])"
        };
        let mut silent = BTreeMap::new();
        for (i, batch) in uids.chunks(HEADER_BATCH).enumerate() {
            progress(Progress::Working {
                phase: Phase::Headers,
                done: i * HEADER_BATCH,
                total,
            });
            let reply = conn.uid_fetch(&uid_set(batch), items, None).await?;
            let answered: BTreeSet<u32> = reply
                .messages
                .iter()
                .filter(|f| f.header.is_some())
                .map(|f| f.uid)
                .collect();
            let mut give_up = BTreeSet::new();
            for &uid in batch.iter().filter(|u| !answered.contains(u)) {
                let before = wanted.get(&uid).copied().unwrap_or(0);
                let now = before + 1;
                if now >= SILENT_TRIES {
                    tracing::warn!(
                        "IMAP: no headers came for UID {uid} in {:?} of {account} in {now} \
                         passes; it is kept as a message that cannot be read",
                        state.name
                    );
                    give_up.insert(uid);
                } else {
                    tracing::debug!(
                        "IMAP: no headers came for UID {uid} in {:?} of {account}",
                        state.name
                    );
                    silent.insert(uid, now);
                }
            }
            let (a, s) = (account.clone(), state.clone());
            let fetched = reply.messages;
            let stored = self
                .blocking(move |store| store_batch(store, &a, folder(&s), fetched, &give_up))
                .await?;
            report.headers += stored.iter().filter(|s| **s == Stored::New).count();
            report.relabelled += stored.iter().filter(|s| **s == Stored::Known).count();
        }
        if total > 0 {
            progress(Progress::Working {
                phase: Phase::Headers,
                done: total,
                total,
            });
        }
        Ok(silent)
    }
}

/// Where a folder's copies are.
pub(super) fn folder(state: &ImapFolder) -> Folder<'_> {
    Folder {
        label: &state.label_id,
        name: &state.name,
        uidvalidity: state.uidvalidity,
    }
}

/// Stores the copies of a batch, newest first: those the server answered
/// with their headers, and those in `give_up` without them (with what
/// the server did say of them, if anything). The others are left for a
/// later pass (one that went while the batch was read, say).
fn store_batch(
    store: &postrinch_store::Store,
    account: &EmailAddress,
    folder: Folder<'_>,
    fetched: Vec<Fetched>,
    give_up: &BTreeSet<u32>,
) -> SyncResult<Vec<Stored>> {
    let mut by_uid: BTreeMap<u32, Fetched> = fetched.into_iter().map(|f| (f.uid, f)).collect();
    for &uid in give_up {
        by_uid.entry(uid).or_insert_with(|| Fetched {
            uid,
            ..Fetched::default()
        });
    }
    let mut stored = Vec::new();
    for f in by_uid.values().rev() {
        let header = match f.header.as_deref() {
            Some(header) => header,
            None if give_up.contains(&f.uid) => &[],
            None => continue,
        };
        stored.push(places::store_copy(store, account, folder, f, header)?);
    }
    Ok(stored)
}
