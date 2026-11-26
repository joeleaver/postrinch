//! What waits before it goes: messages that lost their last copy, and
//! folders missing from LIST.
//!
//! A message whose last copy goes is not removed at once: mail moved by
//! another client vanishes from one folder before it is found in the
//! next, and removing it in between would throw its downloaded body away.
//! Its labels go at once, so it leaves every view, and its id is kept as
//! pending in the account's sync cursor, with the number of the pass it
//! became pending in, *before* its places go, so an interrupted pass
//! leaves nothing behind.
//!
//! It is removed only once every folder has been read whole in a pass
//! after the one it became pending in, and still no copy of it is found:
//! every folder was read after the move (mail moved into a folder the
//! same pass had already read is found by the next one). A folder is not
//! read whole when it refused `SELECT`, when a command in it was refused,
//! when a message in it went unanswered, or when it is missing from LIST
//! for the first time: some servers leave a folder out of a listing for
//! a moment. A folder that stays unreadable stops holding removals back
//! after a few passes ([`super::reads`]). A folder missing from two
//! passes' LISTs is gone: its label and state go in the second, and its
//! mail becomes pending like any other.

use std::collections::{BTreeMap, BTreeSet};

use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};
use postrinch_store::Store;

use super::cursor;
use super::places::{refresh, summary};
use crate::SyncResult;

/// Starts a pass: returns its number, one more than the last one's.
pub(super) fn begin_pass(store: &Store, account: &EmailAddress) -> SyncResult<u64> {
    cursor::update(store, account, |c| {
        c.pass += 1;
        Ok(c.pass)
    })
}

/// Makes these messages pending as of the running pass (again, if they
/// already were: a message that came back and went again waits anew).
pub(super) fn mark<'a>(
    store: &Store,
    account: &EmailAddress,
    ids: impl IntoIterator<Item = &'a ProviderMessageId>,
) -> SyncResult<()> {
    let mut ids = ids.into_iter().peekable();
    if ids.peek().is_none() {
        return Ok(());
    }
    cursor::update(store, account, |c| {
        let pass = c.pass;
        c.pending.extend(ids.map(|id| (id.0.clone(), pass)));
        Ok(())
    })
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
    cursor::update(store, account, |c| {
        c.missing.retain(|id, _| missing.contains(id));
        for id in missing {
            c.missing.entry(id.clone()).or_insert(pass);
        }
        Ok(c.missing
            .iter()
            .filter(|&(_, since)| *since < pass)
            .map(|(id, _)| id.clone())
            .collect())
    })
}

/// At the end of a pass: removes each message pending since a pass before
/// `floor` (every folder was read whole after it became pending,
/// [`super::reads::settle`]) that still has no live copy, and stops
/// waiting for the ones that have one again. Returns how many went.
pub(super) fn resolve(store: &Store, account: &EmailAddress, floor: u64) -> SyncResult<usize> {
    cursor::update(store, account, |cursor| {
        let mut removed = 0;
        let mut waiting = BTreeMap::new();
        for (id, since) in std::mem::take(&mut cursor.pending) {
            let message = ProviderMessageId(id);
            let places = store.imap_message_places(account, &message)?;
            if summary(&places).is_some() || !store.has_message(account, &message)? {
                continue;
            }
            if since < floor {
                store.remove_message(account, &message)?;
                removed += 1;
            } else {
                waiting.insert(message.0, since);
            }
        }
        cursor.pending = waiting;
        Ok(removed)
    })
}
