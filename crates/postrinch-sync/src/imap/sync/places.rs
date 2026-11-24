//! Keeping messages in step with their copies on the server (docs/imap.md,
//! "A message's identity"): a message's labels are the folders holding a
//! copy of it, it is unread if any copy lacks `\Seen` and starred if any
//! has `\Flagged`, and a copy marked `\Deleted` counts as gone.
//!
//! A message whose last copy goes is not removed at once: mail moved by
//! another client vanishes from one folder before it is found in the next,
//! and removing it in between would throw its downloaded body away. Its
//! labels go at once, so it leaves every view, and its id is kept as
//! pending in the account's sync cursor *before* its places go, so an
//! interrupted pass leaves nothing behind. Once a pass has read every
//! folder, each pending message still without a copy is removed
//! ([`resolve_pending`]).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use postrinch_model::{EmailAddress, Flags, ImapPlace, ProviderMessageId, Timestamp};
use postrinch_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

use super::wire::Fetched;
use crate::imap::{Own, message_id, message_id_without_header, thread_for};
use crate::{Envelope, SyncError, SyncResult, parse_headers};

pub(super) const SEEN: &str = "\\Seen";
pub(super) const FLAGGED: &str = "\\Flagged";
pub(super) const DELETED: &str = "\\Deleted";

/// What the sync cursor of an IMAP account holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cursor {
    /// Messages that lost their last copy during an unfinished pass.
    #[serde(default)]
    pending: BTreeSet<String>,
}

fn read_cursor(store: &Store, account: &EmailAddress) -> SyncResult<Cursor> {
    match store.sync_cursor(account)? {
        None => Ok(Cursor::default()),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|e| SyncError::Mime(format!("the IMAP sync cursor of {account}: {e}"))),
    }
}

fn write_cursor(store: &Store, account: &EmailAddress, cursor: &Cursor) -> SyncResult<()> {
    let raw = if cursor.pending.is_empty() {
        None
    } else {
        Some(serde_json::to_string(cursor).map_err(|e| SyncError::Mime(e.to_string()))?)
    };
    Ok(store.set_sync_cursor(account, raw.as_deref())?)
}

/// A copy's flags as kept: sorted, without the session-only `\Recent`.
pub(super) fn normalize(flags: &[String]) -> Vec<String> {
    let mut flags: Vec<String> = flags
        .iter()
        .filter(|f| !f.eq_ignore_ascii_case("\\Recent"))
        .cloned()
        .collect();
    flags.sort();
    flags.dedup();
    flags
}

fn has(place: &ImapPlace, flag: &str) -> bool {
    place.flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
}

/// A message's labels and flags from its copies; `None` when no copy is
/// left that is not marked `\Deleted`.
fn summary(places: &[ImapPlace]) -> Option<(Vec<String>, Flags)> {
    let live: Vec<&ImapPlace> = places.iter().filter(|p| !has(p, DELETED)).collect();
    if live.is_empty() {
        return None;
    }
    let labels: BTreeSet<String> = live.iter().map(|p| p.folder.clone()).collect();
    let flags = Flags {
        unread: live.iter().any(|p| !has(p, SEEN)),
        starred: live.iter().any(|p| has(p, FLAGGED)),
    };
    Some((labels.into_iter().collect(), flags))
}

/// Brings a stored message's labels and flags in step with its copies.
/// Without a live copy it loses its labels and stays pending. Returns
/// whether the message is stored.
fn refresh(store: &Store, account: &EmailAddress, id: &ProviderMessageId) -> SyncResult<bool> {
    if !store.has_message(account, id)? {
        return Ok(false);
    }
    let places = store.imap_message_places(account, id)?;
    let (labels, flags) = summary(&places).unwrap_or_default();
    store.set_message_labels(account, id, &labels, flags)?;
    Ok(true)
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
    let mut cursor = read_cursor(store, account)?;
    cursor
        .pending
        .extend(messages.iter().map(|id| id.0.clone()));
    write_cursor(store, account, &cursor)?;
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

/// Removes each pending message that still has no live copy, now that a
/// pass has read every folder. Returns how many went.
pub(super) fn resolve_pending(store: &Store, account: &EmailAddress) -> SyncResult<usize> {
    let cursor = read_cursor(store, account)?;
    let mut removed = 0;
    for id in &cursor.pending {
        let id = ProviderMessageId(id.clone());
        let places = store.imap_message_places(account, &id)?;
        if summary(&places).is_none() && store.has_message(account, &id)? {
            store.remove_message(account, &id)?;
            removed += 1;
        }
    }
    if !cursor.pending.is_empty() {
        write_cursor(store, account, &Cursor::default())?;
    }
    Ok(removed)
}

/// What became of a copy brought in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stored {
    /// A message new to the store, stored from its headers.
    New,
    /// Another copy of a message already stored.
    Known,
    /// Its only copies are marked `\Deleted`: not stored.
    Skipped,
}

/// Where a copy is: its folder's label id and UIDVALIDITY.
#[derive(Debug, Clone, Copy)]
pub(super) struct Folder<'a> {
    pub label: &'a str,
    pub uidvalidity: u32,
}

/// Stores a copy fetched with its headers: its place, and its message
/// (new, or another copy of one already stored, keeping its thread).
pub(super) fn store_copy(
    store: &Store,
    account: &EmailAddress,
    folder: Folder<'_>,
    fetched: &Fetched,
) -> SyncResult<Stored> {
    let header = fetched.header.as_deref().ok_or_else(|| {
        SyncError::Mime(format!(
            "the server sent no headers for UID {} in {}",
            fetched.uid, folder.label
        ))
    })?;
    let received = fetched
        .internal_date
        .as_deref()
        .and_then(internal_date)
        .unwrap_or_else(now);
    let envelope = Envelope {
        account: account.clone(),
        id: ProviderMessageId(String::new()),
        thread: postrinch_model::ProviderThreadId(String::new()),
        received,
        labels: Vec::new(),
        flags: Flags::default(),
        snippet: None,
    };
    let mut message = parse_headers(envelope, header, u64::from(fetched.size.unwrap_or(0)))?;
    let header_id = message
        .headers
        .message_id
        .clone()
        .filter(|id| !id.trim().is_empty());
    message.id = match &header_id {
        Some(header) => message_id(account, header),
        None => message_id_without_header(folder.uidvalidity, folder.label, fetched.uid),
    };
    let own = match &header_id {
        Some(header) => Own::MessageId(header),
        None => Own::Headerless(&message.id),
    };
    let failed = RefCell::new(None::<StoreError>);
    let thread = thread_for(
        account,
        &message.headers.references,
        message.headers.in_reply_to.as_deref(),
        own,
        |id| match store.message(account, id) {
            Ok(found) => found.map(|m| m.thread),
            Err(e) => {
                failed.borrow_mut().get_or_insert(e);
                None
            }
        },
    );
    if let Some(e) = failed.into_inner() {
        return Err(e.into());
    }
    message.thread = thread;
    store.put_imap_place(
        account,
        &ImapPlace {
            folder: folder.label.to_owned(),
            uidvalidity: folder.uidvalidity,
            uid: fetched.uid,
            message: message.id.clone(),
            flags: normalize(fetched.flags.as_deref().unwrap_or_default()),
            modseq: fetched.modseq.unwrap_or(0),
        },
    )?;
    let places = store.imap_message_places(account, &message.id)?;
    let Some((labels, flags)) = summary(&places) else {
        return Ok(Stored::Skipped);
    };
    let known = store.has_message(account, &message.id)?;
    message.labels = labels;
    message.flags = flags;
    store.put_message(&message, None)?;
    Ok(if known { Stored::Known } else { Stored::New })
}

/// A copy's flags as the server has them now.
#[derive(Debug, Clone)]
pub(super) struct FlagChange {
    pub uid: u32,
    pub flags: Vec<String>,
    pub modseq: Option<u64>,
}

/// What applying flag changes did.
#[derive(Debug, Default)]
pub(super) struct Applied {
    /// Stored messages whose labels or flags were rewritten.
    pub relabelled: usize,
    /// UIDs to fetch by their headers: copies not kept, or kept for a
    /// message the store does not have (its other copies were `\Deleted`).
    pub fetch: Vec<u32>,
}

/// Applies the flags the server reported for copies in a folder.
pub(super) fn apply_flags(
    store: &Store,
    account: &EmailAddress,
    folder: Folder<'_>,
    changes: &[FlagChange],
) -> SyncResult<Applied> {
    let mut kept: HashMap<u32, ImapPlace> = store
        .imap_folder_places(account, folder.label)?
        .into_iter()
        .filter(|p| p.uidvalidity == folder.uidvalidity)
        .map(|p| (p.uid, p))
        .collect();
    let mut applied = Applied::default();
    for change in changes {
        let Some(place) = kept.get_mut(&change.uid) else {
            applied.fetch.push(change.uid);
            continue;
        };
        let flags = normalize(&change.flags);
        let stored = store.has_message(account, &place.message)?;
        if place.flags == flags && stored {
            continue;
        }
        place.flags = flags;
        place.modseq = change.modseq.unwrap_or(place.modseq);
        store.put_imap_place(account, place)?;
        if !stored {
            if !has(place, DELETED) {
                applied.fetch.push(change.uid);
            }
            continue;
        }
        if summary(&store.imap_message_places(account, &place.message)?).is_none() {
            // Every copy is now marked deleted: it waits for the end of the
            // pass, as a vanished one does.
            let mut cursor = read_cursor(store, account)?;
            cursor.pending.insert(place.message.0.clone());
            write_cursor(store, account, &cursor)?;
        }
        refresh(store, account, &place.message)?;
        applied.relabelled += 1;
    }
    Ok(applied)
}

/// The places kept in a folder, split into those of its current
/// UIDVALIDITY (by UID) and those of an earlier one.
pub(super) fn folder_places(
    store: &Store,
    account: &EmailAddress,
    folder: Folder<'_>,
) -> SyncResult<(HashMap<u32, ImapPlace>, Vec<ImapPlace>)> {
    let mut current = HashMap::new();
    let mut stale = Vec::new();
    for place in store.imap_folder_places(account, folder.label)? {
        if place.uidvalidity == folder.uidvalidity {
            current.insert(place.uid, place);
        } else {
            stale.push(place);
        }
    }
    Ok((current, stale))
}

/// IMAP's `INTERNALDATE`, as `17-Jul-1996 02:44:25 -0700`.
pub(super) fn internal_date(text: &str) -> Option<Timestamp> {
    let parsed = jiff::fmt::strtime::parse("%d-%b-%Y %H:%M:%S %z", text.trim()).ok()?;
    let timestamp = parsed.to_timestamp().ok()?;
    Some(Timestamp::from_millis(timestamp.as_millisecond()))
}

pub(super) fn now() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Timestamp::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(folder: &str, flags: &[&str]) -> ImapPlace {
        ImapPlace {
            folder: folder.into(),
            uidvalidity: 1,
            uid: 1,
            message: ProviderMessageId("m1".into()),
            flags: flags.iter().map(|f| f.to_string()).collect(),
            modseq: 0,
        }
    }

    #[test]
    fn copies_make_labels_and_flags() {
        let places = [
            place("INBOX", &[SEEN]),
            place("SENT", &[SEEN, FLAGGED]),
            place("TRASH", &[DELETED]),
        ];
        let (labels, flags) = summary(&places).unwrap();
        assert_eq!(labels, ["INBOX", "SENT"]);
        assert!(!flags.unread);
        assert!(flags.starred);
        let (_, flags) = summary(&[place("INBOX", &[]), place("SENT", &[SEEN])]).unwrap();
        assert!(flags.unread);
        assert!(summary(&[place("INBOX", &[SEEN, DELETED])]).is_none());
    }

    #[test]
    fn internal_dates_parse() {
        let t = internal_date("17-Jul-1996 02:44:25 -0700").unwrap();
        assert_eq!(t.as_millis(), 837_596_665_000);
        assert!(internal_date(" 7-Jul-1996 02:44:25 +0000").is_some());
        assert!(internal_date("yesterday").is_none());
    }

    #[test]
    fn recent_is_not_kept() {
        let flags = normalize(&["\\Seen".into(), "\\Recent".into(), "$Label1".into()]);
        assert_eq!(flags, ["$Label1", "\\Seen"]);
    }
}
