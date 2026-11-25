//! What waits for a complete pass before it goes: messages that lost
//! their last copy, and folders missing from LIST.
//!
//! A message whose last copy goes is not removed at once: mail moved by
//! another client vanishes from one folder before it is found in the
//! next, and removing it in between would throw its downloaded body away.
//! Its labels go at once, so it leaves every view, and its id is kept as
//! pending in the account's sync cursor, with the number of the pass it
//! became pending in, *before* its places go, so an interrupted pass
//! leaves nothing behind.
//!
//! It is removed only by a later *complete* pass that still finds no copy
//! of it: one that started after it became pending, so every folder was
//! read after the move (mail moved into a folder the same pass had
//! already read is found by the next one), and that read every folder it
//! knows. A pass is incomplete when a folder refused `SELECT`, when the
//! pass stopped on an error, or when a folder known from earlier passes
//! is missing from LIST for the first time: some servers leave a folder
//! out of a listing for a moment. A folder missing from two passes' LISTs
//! is gone: its label and state go in the second, and its mail becomes
//! pending like any other.

use std::collections::{BTreeMap, BTreeSet};

use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};
use postrinch_store::Store;
use serde::{Deserialize, Serialize};

use super::places::{refresh, summary};
use crate::{SyncError, SyncResult};

/// What the sync cursor of an IMAP account holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cursor {
    /// The number of the pass running, or of the last one.
    #[serde(default)]
    pass: u64,
    /// Messages without a live copy, with the pass each became so in.
    #[serde(default)]
    pending: BTreeMap<String, u64>,
    /// Label ids of folders missing from LIST, with the pass each was
    /// first missed in.
    #[serde(default)]
    missing: BTreeMap<String, u64>,
}

fn read(store: &Store, account: &EmailAddress) -> SyncResult<Cursor> {
    match store.sync_cursor(account)? {
        None => Ok(Cursor::default()),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|e| SyncError::Mime(format!("the IMAP sync cursor of {account}: {e}"))),
    }
}

fn write(store: &Store, account: &EmailAddress, cursor: &Cursor) -> SyncResult<()> {
    let raw = serde_json::to_string(cursor).map_err(|e| SyncError::Mime(e.to_string()))?;
    Ok(store.set_sync_cursor(account, Some(&raw))?)
}

/// Starts a pass: returns its number, one more than the last one's.
pub(super) fn begin_pass(store: &Store, account: &EmailAddress) -> SyncResult<u64> {
    let mut cursor = read(store, account)?;
    cursor.pass += 1;
    write(store, account, &cursor)?;
    Ok(cursor.pass)
}

/// Makes these messages pending as of the running pass (again, if they
/// already were: a message that came back and went again waits anew).
pub(super) fn mark<'a>(
    store: &Store,
    account: &EmailAddress,
    ids: impl IntoIterator<Item = &'a ProviderMessageId>,
) -> SyncResult<()> {
    let mut cursor = read(store, account)?;
    let pass = cursor.pass;
    cursor
        .pending
        .extend(ids.into_iter().map(|id| (id.0.clone(), pass)));
    write(store, account, &cursor)
}

/// Forgets copies the server no longer has (or that belong to another
/// folder): their messages are made pending first, then the places go,
/// then each message is brought in step with what is left.
pub(super) fn forget(
    store: &Store,
    account: &EmailAddress,
    places: &[ImapPlace],
) -> SyncResult<()> {
    if places.is_empty() {
        return Ok(());
    }
    let messages: BTreeSet<&ProviderMessageId> = places.iter().map(|p| &p.message).collect();
    mark(store, account, messages.iter().copied())?;
    for place in places {
        store.remove_imap_place(account, &place.folder, place.uidvalidity, place.uid)?;
    }
    for id in messages {
        refresh(store, account, id)?;
    }
    Ok(())
}

/// Forgets every copy kept under a folder's label, as when the folder
/// went or its role moved to a folder of another name.
pub(super) fn forget_folder(store: &Store, account: &EmailAddress, label: &str) -> SyncResult<()> {
    let places = store.imap_folder_places(account, label)?;
    forget(store, account, &places)
}

/// Notes which known folders (by label id) LIST left out in pass `pass`.
/// Returns those it also left out in an earlier pass, so are gone; the
/// others are missing for the first time.
pub(super) fn note_missing(
    store: &Store,
    account: &EmailAddress,
    missing: &BTreeSet<String>,
    pass: u64,
) -> SyncResult<Vec<String>> {
    let mut cursor = read(store, account)?;
    cursor.missing.retain(|id, _| missing.contains(id));
    for id in missing {
        cursor.missing.entry(id.clone()).or_insert(pass);
    }
    let gone = cursor
        .missing
        .iter()
        .filter(|&(_, since)| *since < pass)
        .map(|(id, _)| id.clone())
        .collect();
    write(store, account, &cursor)?;
    Ok(gone)
}

/// At the end of complete pass `pass`: removes each message pending since
/// an earlier pass that still has no live copy, and stops waiting for the
/// ones that have one again. Returns how many went.
pub(super) fn resolve(store: &Store, account: &EmailAddress, pass: u64) -> SyncResult<usize> {
    let mut cursor = read(store, account)?;
    let mut removed = 0;
    let mut waiting = BTreeMap::new();
    for (id, since) in std::mem::take(&mut cursor.pending) {
        let message = ProviderMessageId(id);
        let places = store.imap_message_places(account, &message)?;
        if summary(&places).is_some() || !store.has_message(account, &message)? {
            continue;
        }
        if since < pass {
            store.remove_message(account, &message)?;
            removed += 1;
        } else {
            waiting.insert(message.0, since);
        }
    }
    cursor.pending = waiting;
    write(store, account, &cursor)?;
    Ok(removed)
}
