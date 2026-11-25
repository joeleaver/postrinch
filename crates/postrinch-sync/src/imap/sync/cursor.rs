//! The account's sync cursor: what IMAP sync keeps between passes beyond
//! each folder's own state. It is small JSON in the account's record:
//! what is pending ([`super::pending`]), how each folder was read
//! ([`super::reads`]), and the copies changes left marked `\Deleted`
//! ([`super::changes`]). Each part stays bounded: pending messages are
//! resolved as folders are read, folders that went are dropped, a message
//! the server will not answer for is given up on after a few passes, and
//! marked copies are dropped once they are no longer marked.

use std::collections::{BTreeMap, BTreeSet};

use postrinch_model::EmailAddress;
use postrinch_store::Store;
use serde::{Deserialize, Serialize};

use crate::{SyncError, SyncResult};

/// What the sync cursor of an IMAP account holds.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Cursor {
    /// The number of the pass running, or of the last one.
    #[serde(default)]
    pub pass: u64,
    /// Messages without a live copy, with the pass each became so in.
    #[serde(default)]
    pub pending: BTreeMap<String, u64>,
    /// Label ids of folders missing from LIST, with the pass each was
    /// first missed in.
    #[serde(default)]
    pub missing: BTreeMap<String, u64>,
    /// How each folder (by label id) was read.
    #[serde(default)]
    pub folders: BTreeMap<String, FolderRead>,
    /// Per folder (by label id), the messages the server lists but gave
    /// no headers for.
    #[serde(default)]
    pub silent: BTreeMap<String, Silent>,
    /// Per folder (by label id), the copies a change marked `\Deleted` and
    /// left unexpunged, as another client's marked mail was there
    /// (docs/imap.md, "Keeping every message safe").
    #[serde(default)]
    pub marked: BTreeMap<String, Marked>,
}

/// How a folder was read over the passes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FolderRead {
    /// The last pass that read it whole; 0 for none.
    #[serde(default)]
    pub whole: u64,
    /// How many passes in a row did not read it whole (it refused SELECT,
    /// a command in it was refused, or it was missing from LIST).
    #[serde(default)]
    pub failing: u32,
}

/// The messages of one folder the server did not answer for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Silent {
    /// The folder's UIDVALIDITY the UIDs belong to.
    pub uidvalidity: u32,
    /// Each UID, with how many passes it went unanswered in.
    pub tries: BTreeMap<u32, u32>,
}

/// The copies of one folder a change left marked `\Deleted`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Marked {
    /// The folder's UIDVALIDITY the UIDs belong to.
    pub uidvalidity: u32,
    pub uids: BTreeSet<u32>,
}

pub(super) fn read(store: &Store, account: &EmailAddress) -> SyncResult<Cursor> {
    match store.sync_cursor(account)? {
        None => Ok(Cursor::default()),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|e| SyncError::Mime(format!("the IMAP sync cursor of {account}: {e}"))),
    }
}

pub(super) fn write(store: &Store, account: &EmailAddress, cursor: &Cursor) -> SyncResult<()> {
    let raw = serde_json::to_string(cursor).map_err(|e| SyncError::Mime(e.to_string()))?;
    Ok(store.set_sync_cursor(account, Some(&raw))?)
}

/// Reads the cursor, changes it and writes it back.
pub(super) fn update<T>(
    store: &Store,
    account: &EmailAddress,
    change: impl FnOnce(&mut Cursor) -> T,
) -> SyncResult<T> {
    let mut cursor = read(store, account)?;
    let out = change(&mut cursor);
    write(store, account, &cursor)?;
    Ok(out)
}
