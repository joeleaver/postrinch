//! Keeping messages in step with their copies on the server (docs/imap.md,
//! "A message's identity"): a message's labels are the folders holding a
//! copy of it that are labels (all but Gmail's All Mail,
//! [`super::folders::holds_everything`]), it is unread if any copy lacks
//! `\Seen` and starred if any has `\Flagged`, and a copy marked `\Deleted`
//! counts as gone. A message
//! left without a live copy waits until every folder is read again
//! ([`super::pending`]).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use postrinch_model::{
    EmailAddress, Flags, ImapPlace, Message, MessageHeaders, ProviderMessageId, ProviderThreadId,
    Timestamp,
};
use postrinch_store::{Store, StoreError};

use super::pending;
use super::wire::Fetched;
use crate::imap::{Own, message_id, message_id_without_header, thread_for};
use crate::{Envelope, SyncResult, parse_headers};

pub(super) const SEEN: &str = "\\Seen";
pub(super) const FLAGGED: &str = "\\Flagged";
pub(super) const DELETED: &str = "\\Deleted";

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

/// A message's folders and flags from its copies; `None` when no copy is
/// left that is not marked `\Deleted`. Its labels are those of the
/// folders that are labels ([`labelled`]).
pub(super) fn summary(places: &[ImapPlace]) -> Option<(Vec<String>, Flags)> {
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

/// Of a message's folders (label ids), those that are labels: a folder
/// the pass keeps no label for (Gmail's All Mail, which holds every
/// message) puts none on its messages.
fn labelled(
    store: &Store,
    account: &EmailAddress,
    folders: Vec<String>,
) -> SyncResult<Vec<String>> {
    let mut labels = Vec::with_capacity(folders.len());
    for folder in folders {
        if store.has_label(account, &folder)? {
            labels.push(folder);
        }
    }
    Ok(labels)
}

/// Brings a stored message's labels and flags in step with its copies.
/// Without a live copy it loses its labels and stays pending. Returns
/// whether the message is stored.
pub(super) fn refresh(
    store: &Store,
    account: &EmailAddress,
    id: &ProviderMessageId,
) -> SyncResult<bool> {
    if !store.has_message(account, id)? {
        return Ok(false);
    }
    let places = store.imap_message_places(account, id)?;
    let (folders, flags) = summary(&places).unwrap_or_default();
    let labels = labelled(store, account, folders)?;
    store.set_message_labels(account, id, &labels, flags)?;
    Ok(true)
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

/// Where a copy is: its folder's label id, name on the server (for what
/// is logged) and UIDVALIDITY.
#[derive(Debug, Clone, Copy)]
pub(super) struct Folder<'a> {
    pub label: &'a str,
    pub name: &'a str,
    pub uidvalidity: u32,
}

/// Stores a copy fetched with its headers: its place, and its message
/// (new, or another copy of one already stored, keeping its thread).
///
/// Headers that cannot be read (a zero-byte message, as maildir servers
/// can hold, or a header block the parser cannot make sense of) do not
/// stop sync: the copy is stored as a message of its own with what the
/// server says of it (its time from INTERNALDATE, its size, no subject),
/// named by its place as a message without a `Message-ID` is, so the
/// person sees it is there. Backfill downloads it once like any other.
///
/// When several copies of one message are stored by their headers, the
/// first of them ([`first_copy`]) gives the message its time and headers,
/// whatever order they are read in.
pub(super) fn store_copy(
    store: &Store,
    account: &EmailAddress,
    folder: Folder<'_>,
    fetched: &Fetched,
    header: &[u8],
) -> SyncResult<Stored> {
    let internal = fetched.internal_date.as_deref().and_then(internal_date);
    let received = internal.unwrap_or_else(now);
    let size = u64::from(fetched.size.unwrap_or(0));
    let envelope = Envelope {
        account: account.clone(),
        id: ProviderMessageId(String::new()),
        thread: ProviderThreadId(String::new()),
        received,
        labels: Vec::new(),
        flags: Flags::default(),
        snippet: None,
    };
    let mut message = match parse_headers(envelope.clone(), header, size) {
        Ok(message) => message,
        Err(e) => {
            tracing::warn!(
                "IMAP: the headers of UID {} in {:?} of {account} cannot be read ({e}); \
                 it is kept without them",
                fetched.uid,
                folder.name
            );
            unreadable(envelope, size)
        }
    };
    let header_id = message
        .headers
        .message_id
        .clone()
        .filter(|id| !id.trim().is_empty());
    message.id = match &header_id {
        Some(header) => message_id(account, header),
        None => message_id_without_header(folder.uidvalidity, folder.label, fetched.uid),
    };
    store.put_imap_place(
        account,
        &ImapPlace {
            folder: folder.label.to_owned(),
            uidvalidity: folder.uidvalidity,
            uid: fetched.uid,
            message: message.id.clone(),
            flags: normalize(fetched.flags.as_deref().unwrap_or_default()),
            modseq: fetched.modseq.unwrap_or(0),
            arrived: internal,
        },
    )?;
    let places = store.imap_message_places(account, &message.id)?;
    let Some((folders, flags)) = summary(&places) else {
        return Ok(Stored::Skipped);
    };
    let labels = labelled(store, account, folders)?;
    if let Some(kept) = store.message(account, &message.id)? {
        let first = first_copy(&places).is_some_and(|p| {
            p.folder == folder.label && p.uidvalidity == folder.uidvalidity && p.uid == fetched.uid
        });
        if internal.is_none() {
            message.received = kept.received;
        }
        let same = kept.received == message.received && kept.headers == message.headers;
        if !first || same {
            store.set_message_labels(account, &message.id, &labels, flags)?;
            return Ok(Stored::Known);
        }
        // The first copy: its time and headers, the message's thread.
        message.thread = kept.thread;
        message.labels = labels;
        message.flags = flags;
        store.put_message(&message, None)?;
        return Ok(Stored::Known);
    }
    let own = match &header_id {
        Some(header) => Own::MessageId(header),
        None => Own::Headerless(&message.id),
    };
    message.thread = thread(store, account, &message.headers, own)?;
    message.labels = labels;
    message.flags = flags;
    store.put_message(&message, None)?;
    Ok(Stored::New)
}

/// Of a message's live copies, the one that gives it its time, headers
/// and body: the one that arrived first (the earliest INTERNALDATE; that
/// is when the message came, wherever another client filed it later),
/// copies without one last, then by folder label id and UID, so the
/// choice is the same whatever order the copies are read in.
pub(super) fn first_copy<'a>(
    places: impl IntoIterator<Item = &'a ImapPlace>,
) -> Option<&'a ImapPlace> {
    live_copies(places).into_iter().next()
}

/// A message's live copies, in the order [`first_copy`] picks from.
pub(super) fn live_copies<'a>(
    places: impl IntoIterator<Item = &'a ImapPlace>,
) -> Vec<&'a ImapPlace> {
    let mut live: Vec<&ImapPlace> = places.into_iter().filter(|p| !has(p, DELETED)).collect();
    live.sort_by_key(|p| {
        (
            p.arrived.is_none(),
            p.arrived,
            &p.folder,
            p.uidvalidity,
            p.uid,
        )
    });
    live
}

/// The thread of a message new to the store.
fn thread(
    store: &Store,
    account: &EmailAddress,
    headers: &MessageHeaders,
    own: Own<'_>,
) -> SyncResult<ProviderThreadId> {
    let failed = RefCell::new(None::<StoreError>);
    let thread = thread_for(
        account,
        &headers.references,
        headers.in_reply_to.as_deref(),
        own,
        |id| match store.message(account, id) {
            Ok(found) => found.map(|m| m.thread),
            Err(e) => {
                failed.borrow_mut().get_or_insert(e);
                None
            }
        },
    );
    match failed.into_inner() {
        Some(e) => Err(e.into()),
        None => Ok(thread),
    }
}

/// A message whose headers cannot be read: what the server says of it.
fn unreadable(envelope: Envelope, size: u64) -> Message {
    Message {
        account: envelope.account,
        id: envelope.id,
        thread: envelope.thread,
        headers: MessageHeaders::default(),
        received: envelope.received,
        labels: envelope.labels,
        flags: envelope.flags,
        snippet: String::new(),
        body_text: String::new(),
        attachments: Vec::new(),
        size,
        downloaded: false,
    }
}

/// A copy's flags as the server has them now.
#[derive(Debug, Clone)]
pub(super) struct FlagChange {
    pub uid: u32,
    /// `None` when the server's answer carried no FLAGS: the copy is
    /// there, and nothing is known of its flags.
    pub flags: Option<Vec<String>>,
    pub modseq: Option<u64>,
}

impl FlagChange {
    pub(super) fn of(fetched: &Fetched) -> Self {
        Self {
            uid: fetched.uid,
            flags: fetched.flags.clone(),
            modseq: fetched.modseq,
        }
    }
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

impl Applied {
    pub(super) fn add(&mut self, other: Applied) {
        self.relabelled += other.relabelled;
        self.fetch.extend(other.fetch);
    }
}

/// Applies the flags the server reported for copies in a folder, given
/// the places kept there of its current UIDVALIDITY, by UID (kept up to
/// date as they change).
pub(super) fn apply_flags(
    store: &Store,
    account: &EmailAddress,
    kept: &mut HashMap<u32, ImapPlace>,
    changes: &[FlagChange],
) -> SyncResult<Applied> {
    let mut applied = Applied::default();
    let mut emptied = Vec::new();
    for change in changes {
        let Some(place) = kept.get_mut(&change.uid) else {
            applied.fetch.push(change.uid);
            continue;
        };
        let Some(flags) = change.flags.as_deref() else {
            continue;
        };
        let flags = normalize(flags);
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
            // Every copy is now marked deleted: it waits as a vanished
            // one does (made pending below, before it loses its labels).
            emptied.push(place.message.clone());
            continue;
        }
        refresh(store, account, &place.message)?;
        applied.relabelled += 1;
    }
    pending::mark(store, account, &emptied)?;
    for id in &emptied {
        refresh(store, account, id)?;
    }
    applied.relabelled += emptied.len();
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
            arrived: None,
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
    fn the_first_copy_is_the_earliest_then_by_folder_and_uid() {
        let at = |folder: &str, uid, millis: Option<i64>| ImapPlace {
            uid,
            arrived: millis.map(Timestamp::from_millis),
            ..place(folder, &[])
        };
        let copies = [
            at("Work", 3, Some(20)),
            at("INBOX", 9, Some(10)),
            at("Archive", 5, Some(10)),
            at("Archive", 2, None),
        ];
        let first = first_copy(&copies).unwrap();
        assert_eq!((first.folder.as_str(), first.uid), ("Archive", 5));
        // Whatever the order.
        let mut reversed = copies.clone();
        reversed.reverse();
        assert_eq!(first_copy(&reversed), Some(first));
        let deleted = ImapPlace {
            flags: vec![DELETED.into()],
            ..at("A", 1, Some(1))
        };
        assert_eq!(first_copy([&deleted]), None);
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
