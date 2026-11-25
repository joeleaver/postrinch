//! Keeping the store's copies (`ImapPlace`) in step with what a change
//! did on the server, at once, so a change sent again after a failure
//! starts from where the last one stopped and never copies a message
//! twice, and the window shows what the server now has (the outbox's
//! entry still applies over it until a sync has it).
//!
//! A message whose last copy goes from the store here (its new place
//! unknown: a move without UIDPLUS, of a message without a
//! `Message-ID`) is made pending first, as sync does, so it waits for the
//! next pass to find it rather than being lost or shown twice.

use std::collections::HashSet;

use mail_parser::MessageParser;
use postrinch_model::{EmailAddress, ImapPlace, ProviderMessageId};
use postrinch_store::Store;

use super::super::pending;
use super::super::places::{self, DELETED, Folder, normalize, refresh};
use super::super::wire::Fetched;
use crate::SyncResult;
use crate::imap::message_id;

fn same(a: &ImapPlace, b: &ImapPlace) -> bool {
    a.folder == b.folder && a.uidvalidity == b.uidvalidity && a.uid == b.uid
}

/// The kept copy at `place`'s place, as it is now.
fn current(
    store: &Store,
    account: &EmailAddress,
    place: &ImapPlace,
) -> SyncResult<Option<ImapPlace>> {
    Ok(store
        .imap_message_places(account, &place.message)?
        .into_iter()
        .find(|p| same(p, place)))
}

/// Sets or clears `flag` on these kept copies.
pub(super) fn set_flag(
    store: &Store,
    account: &EmailAddress,
    copies: &[ImapPlace],
    flag: &str,
    add: bool,
) -> SyncResult<()> {
    for copy in copies {
        let Some(mut kept) = current(store, account, copy)? else {
            continue;
        };
        kept.flags.retain(|f| !f.eq_ignore_ascii_case(flag));
        if add {
            kept.flags.push(flag.to_owned());
        }
        kept.flags = normalize(&kept.flags);
        store.put_imap_place(account, &kept)?;
    }
    Ok(())
}

/// Keeps the copy a command made of `source` in the folder that is label
/// `folder`: its flags and arrival are the source's (`COPY` and `MOVE`
/// keep both), its MODSEQ unknown until sync reads it.
pub(super) fn add_copy(
    store: &Store,
    account: &EmailAddress,
    source: &ImapPlace,
    folder: &str,
    uidvalidity: u32,
    uid: u32,
) -> SyncResult<()> {
    let from = current(store, account, source)?.unwrap_or_else(|| source.clone());
    let mut flags = from.flags.clone();
    flags.retain(|f| !f.eq_ignore_ascii_case(DELETED));
    store.put_imap_place(
        account,
        &ImapPlace {
            folder: folder.to_owned(),
            uidvalidity,
            uid,
            message: from.message.clone(),
            flags,
            modseq: 0,
            arrived: from.arrived,
        },
    )?;
    Ok(())
}

/// Forgets a copy the server no longer has, making its message pending
/// first when it was the last live one.
pub(super) fn remove_copy(
    store: &Store,
    account: &EmailAddress,
    place: &ImapPlace,
) -> SyncResult<()> {
    let rest: Vec<ImapPlace> = store
        .imap_message_places(account, &place.message)?
        .into_iter()
        .filter(|p| !same(p, place))
        .collect();
    if places::summary(&rest).is_none() && store.has_message(account, &place.message)? {
        pending::mark(store, account, [&place.message])?;
    }
    store
        .remove_imap_place(account, &place.folder, place.uidvalidity, place.uid)
        .map_err(Into::into)
}

/// Keeps the copies of `wanted` messages found among a folder's new
/// mail, by their `Message-ID`: how a copy is found when the server did
/// not say where it put it. Other new mail is left to sync. Returns the
/// messages found.
pub(super) fn found_copies(
    store: &Store,
    account: &EmailAddress,
    folder: Folder<'_>,
    fetched: &[Fetched],
    wanted: &HashSet<ProviderMessageId>,
) -> SyncResult<HashSet<ProviderMessageId>> {
    let mut found = HashSet::new();
    for copy in fetched {
        let Some(header) = copy.header.as_deref() else {
            continue;
        };
        let Some(id) = MessageParser::default()
            .parse(header)
            .and_then(|m| m.message_id().map(str::to_owned))
            .filter(|id| !id.trim().is_empty())
            .map(|id| message_id(account, &id))
        else {
            continue;
        };
        if wanted.contains(&id) {
            places::store_copy(store, account, folder, copy, header)?;
            found.insert(id);
        }
    }
    Ok(found)
}

/// Keeps every copy read from a folder with its headers, as sync keeps
/// new mail: how mail the store does not know yet becomes known before a
/// label's deletion saves it. Returns the UIDs kept.
pub(super) fn store_read(
    store: &Store,
    account: &EmailAddress,
    folder: Folder<'_>,
    fetched: &[Fetched],
) -> SyncResult<HashSet<u32>> {
    let mut kept = HashSet::new();
    for copy in fetched {
        if let Some(header) = copy.header.as_deref() {
            places::store_copy(store, account, folder, copy, header)?;
            kept.insert(copy.uid);
        }
    }
    Ok(kept)
}

/// Brings each message's labels and flags in step with its copies, now
/// that a change is done or stopped.
pub(super) fn settle<'a>(
    store: &Store,
    account: &EmailAddress,
    messages: impl IntoIterator<Item = &'a ProviderMessageId>,
) -> SyncResult<()> {
    for id in messages {
        refresh(store, account, id)?;
    }
    Ok(())
}
