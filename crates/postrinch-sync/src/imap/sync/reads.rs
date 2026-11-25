//! How each folder was read, and the messages a server will not answer
//! for (docs/imap.md, "Sync").
//!
//! A message that lost its last copy is removed once every folder has
//! been read whole since ([`floor`]): until then it may be in one not yet
//! read. Two things would hold that back for ever, and are bounded here:
//!
//! - a folder that is never read whole (it always refuses `SELECT`, say)
//!   stops holding removals back after [`STUCK_AFTER`] passes in a row;
//!   mail moved into it is then taken for gone, and comes back, by its
//!   headers, once the folder can be read again;
//! - a message the server lists but gives no headers for is asked for
//!   again in later passes, by its UID (the folder's position moves on
//!   meanwhile, so nothing else is read twice or skipped), and after
//!   [`SILENT_TRIES`] passes without an answer is stored as a message
//!   that cannot be read, with what the server said of it, as one whose
//!   headers cannot be parsed is ([`super::places::store_copy`]).

use std::collections::{BTreeMap, BTreeSet};

use postrinch_model::EmailAddress;
use postrinch_store::Store;

use super::cursor::{self, Cursor, Silent};
use crate::SyncResult;

/// Passes in a row a folder may go unread before it no longer holds
/// back the removal of mail that went from the others.
pub(super) const STUCK_AFTER: u32 = 5;

/// Passes a message may go unanswered before it is stored as one that
/// cannot be read. A server too busy to answer stops the folder
/// ([`super::folder_error::FolderError::Busy`]) or the pass
/// ([`crate::SyncError::Busy`]) instead, so being busy never counts.
pub(super) const SILENT_TRIES: u32 = 3;

/// Notes how pass `pass` read a folder: whole, or not.
pub(super) fn note(
    store: &Store,
    account: &EmailAddress,
    label: &str,
    pass: u64,
    whole: bool,
) -> SyncResult<()> {
    cursor::update(store, account, |c| note_in(c, label, pass, whole))
}

fn note_in(cursor: &mut Cursor, label: &str, pass: u64, whole: bool) {
    let read = cursor.folders.entry(label.to_owned()).or_default();
    if whole {
        read.whole = pass;
        read.failing = 0;
    } else {
        read.failing = read.failing.saturating_add(1);
    }
}

/// At the end of pass `pass`, with the folders `known` (by label id):
/// forgets what is kept of folders that went, notes the folders `unread`
/// (missing from LIST for the first time) as not read, and returns the
/// floor: the messages pending since a pass before it may go.
pub(super) fn settle(
    store: &Store,
    account: &EmailAddress,
    known: &BTreeSet<String>,
    unread: &BTreeSet<String>,
    pass: u64,
) -> SyncResult<u64> {
    cursor::update(store, account, |c| {
        c.folders.retain(|id, _| known.contains(id));
        c.silent.retain(|id, _| known.contains(id));
        c.marked.retain(|id, _| known.contains(id));
        for id in unread {
            note_in(c, id, pass, false);
        }
        floor(c, known, pass)
    })
}

/// The earliest pass since which every folder that is not stuck has been
/// read whole (`pass`, the one running, when none is left to wait for).
fn floor(cursor: &Cursor, known: &BTreeSet<String>, pass: u64) -> u64 {
    known
        .iter()
        .map(|id| cursor.folders.get(id).copied().unwrap_or_default())
        .filter(|read| read.failing < STUCK_AFTER)
        .map(|read| read.whole)
        .min()
        .unwrap_or(pass)
}

/// The messages of a folder of this UIDVALIDITY that went unanswered,
/// with how many passes each did.
pub(super) fn silent(
    store: &Store,
    account: &EmailAddress,
    label: &str,
    uidvalidity: u32,
) -> SyncResult<BTreeMap<u32, u32>> {
    let cursor = cursor::read(store, account)?;
    Ok(cursor
        .silent
        .get(label)
        .filter(|s| s.uidvalidity == uidvalidity)
        .map(|s| s.tries.clone())
        .unwrap_or_default())
}

/// Keeps the messages of a folder that went unanswered (none: forgets
/// the folder's).
pub(super) fn keep_silent(
    store: &Store,
    account: &EmailAddress,
    label: &str,
    uidvalidity: u32,
    tries: BTreeMap<u32, u32>,
) -> SyncResult<()> {
    let cursor = cursor::read(store, account)?;
    let kept = cursor.silent.get(label);
    let same = match kept {
        None => tries.is_empty(),
        Some(s) => s.uidvalidity == uidvalidity && s.tries == tries,
    };
    if same {
        return Ok(());
    }
    cursor::update(store, account, |c| {
        if tries.is_empty() {
            c.silent.remove(label);
        } else {
            c.silent
                .insert(label.to_owned(), Silent { uidvalidity, tries });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_floor_waits_for_every_folder_but_stuck_ones() {
        let mut c = Cursor::default();
        let all = known(&["INBOX", "Work"]);
        note_in(&mut c, "INBOX", 4, true);
        // Work was never read whole: nothing pending may go.
        assert_eq!(floor(&c, &all, 4), 0);
        note_in(&mut c, "Work", 3, true);
        assert_eq!(floor(&c, &all, 4), 3);
        // Work goes unread pass after pass, and stops holding back.
        for pass in 5..5 + u64::from(STUCK_AFTER) {
            note_in(&mut c, "INBOX", pass, true);
            note_in(&mut c, "Work", pass, false);
        }
        assert_eq!(floor(&c, &all, 10), 9);
        // Read again, it counts again.
        note_in(&mut c, "Work", 10, true);
        note_in(&mut c, "INBOX", 10, true);
        assert_eq!(floor(&c, &all, 10), 10);
        assert_eq!(floor(&c, &known(&[]), 11), 11);
    }
}
