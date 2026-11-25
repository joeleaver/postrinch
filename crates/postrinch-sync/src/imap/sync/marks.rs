//! The copies Postrinch itself marked `\Deleted`, per folder, kept in the
//! sync cursor ([`super::cursor::Cursor::marked`]; docs/imap.md, "Keeping
//! every message safe").
//!
//! A copy is noted here before the `UID STORE` that marks it, by the
//! outbox and by outgoing mail alike, and again once the server took the
//! mark: so whatever comes between the mark and its expunge (a connection
//! lost, a busy answer, a crash, the server refusing to take the mark off
//! again), the mark stays known as ours. Ours are never taken for another
//! client's mail marked mid-delete, which would hold a plain `EXPUNGE`
//! back for ever, and a later expunge in the folder takes them. A UID goes
//! from here once it is expunged, unmarked again by us, or found no longer
//! marked (expunged or unmarked by another client); the whole folder's go
//! when its UIDVALIDITY changes or it goes.

use std::collections::{BTreeSet, HashSet};

use postrinch_model::EmailAddress;
use postrinch_store::Store;

use super::cursor::{self, Marked};
use super::pending;
use crate::SyncResult;

/// Notes `uids` of the folder `label`, at UIDVALIDITY `uidvalidity`, as
/// marked by Postrinch. UIDs noted at another UIDVALIDITY name other
/// messages now, and are dropped.
pub(super) fn note(
    store: &Store,
    account: &EmailAddress,
    (label, uidvalidity): (&str, u32),
    uids: &[u32],
) -> SyncResult<()> {
    if uids.is_empty() {
        return Ok(());
    }
    cursor::update(store, account, |c| {
        let marked = c.marked.entry(label.to_owned()).or_default();
        if marked.uidvalidity != uidvalidity {
            *marked = Marked {
                uidvalidity,
                uids: BTreeSet::new(),
            };
        }
        marked.uids.extend(uids);
        Ok(())
    })
}

/// Drops `uids` of the folder `label` at `uidvalidity`: expunged, or
/// unmarked again.
pub(super) fn forget(
    store: &Store,
    account: &EmailAddress,
    (label, uidvalidity): (&str, u32),
    uids: &[u32],
) -> SyncResult<()> {
    if uids.is_empty() {
        return Ok(());
    }
    cursor::update(store, account, |c| {
        if let Some(marked) = c.marked.get_mut(label)
            && marked.uidvalidity == uidvalidity
        {
            marked.uids.retain(|u| !uids.contains(u));
            if marked.uids.is_empty() {
                c.marked.remove(label);
            }
        }
        Ok(())
    })
}

/// Of the UIDs noted for the folder `label` at `uidvalidity`, those
/// `marked` on the server now: Postrinch's own marks there. The others
/// are dropped, expunged or unmarked by another client since (or not yet
/// marked by a mark in flight, which notes them again once the server
/// takes it).
pub(super) fn ours(
    store: &Store,
    account: &EmailAddress,
    (label, uidvalidity): (&str, u32),
    marked: &HashSet<u32>,
) -> SyncResult<BTreeSet<u32>> {
    cursor::update(store, account, |c| {
        let Some(kept) = c.marked.get_mut(label) else {
            return Ok(BTreeSet::new());
        };
        if kept.uidvalidity != uidvalidity {
            c.marked.remove(label);
            return Ok(BTreeSet::new());
        }
        kept.uids.retain(|u| marked.contains(u));
        let ours = kept.uids.clone();
        if ours.is_empty() {
            c.marked.remove(label);
        }
        Ok(ours)
    })
}

/// After an expunge in the folder `label` at `uidvalidity` that took
/// `gone`, Postrinch's marks there: their copies are forgotten, as sync
/// forgets copies that went, and the UIDs dropped.
pub(super) fn expunged(
    store: &Store,
    account: &EmailAddress,
    (label, uidvalidity): (&str, u32),
    gone: &BTreeSet<u32>,
) -> SyncResult<()> {
    if gone.is_empty() {
        return Ok(());
    }
    let places: Vec<_> = store
        .imap_folder_places(account, label)?
        .into_iter()
        .filter(|p| p.uidvalidity == uidvalidity && gone.contains(&p.uid))
        .collect();
    pending::forget(store, account, &places)?;
    let gone: Vec<u32> = gone.iter().copied().collect();
    forget(store, account, (label, uidvalidity), &gone)
}

/// The folders with marks noted, with their UIDVALIDITY.
pub(super) fn folders(store: &Store, account: &EmailAddress) -> SyncResult<Vec<(String, u32)>> {
    Ok(cursor::read(store, account)?
        .marked
        .into_iter()
        .map(|(label, marked)| (label, marked.uidvalidity))
        .collect())
}

#[cfg(test)]
mod tests {
    use postrinch_model::{Account, Provider};

    use super::*;

    #[test]
    fn marks_are_kept_until_seen_unmarked_and_per_uidvalidity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("ada@example.org").unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Imap))
            .unwrap();
        note(&store, &me, ("INBOX", 7), &[1, 2, 3]).unwrap();
        let marked: HashSet<u32> = [2, 3, 9].into();
        assert_eq!(
            ours(&store, &me, ("INBOX", 7), &marked).unwrap(),
            [2, 3].into()
        );
        // 1 was no longer marked, so it went.
        let all: HashSet<u32> = [1, 2, 3].into();
        assert_eq!(
            ours(&store, &me, ("INBOX", 7), &all).unwrap(),
            [2, 3].into()
        );
        forget(&store, &me, ("INBOX", 7), &[2]).unwrap();
        assert_eq!(folders(&store, &me).unwrap(), [("INBOX".to_owned(), 7)]);
        // Another UIDVALIDITY: the UIDs noted name other messages.
        assert!(ours(&store, &me, ("INBOX", 8), &all).unwrap().is_empty());
        assert!(folders(&store, &me).unwrap().is_empty());
        note(&store, &me, ("INBOX", 7), &[4]).unwrap();
        note(&store, &me, ("INBOX", 8), &[5]).unwrap();
        let both: HashSet<u32> = [4, 5].into();
        assert_eq!(ours(&store, &me, ("INBOX", 8), &both).unwrap(), [5].into());
    }
}
