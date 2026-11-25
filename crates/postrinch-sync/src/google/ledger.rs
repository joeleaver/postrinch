//! Which Google sign-ins are under way for an address, and one turn at a
//! time for the steps that read the keychain to decide about a Google
//! grant (docs/imap.md, "Credentials").
//!
//! A Google account and Google over IMAP share one grant per address, so
//! keeping one kind's token and revoking the other kind's must not
//! interleave. Without this, a Google account's refused sign-in could
//! revoke the grant an IMAP sign-in has exchanged its code for but not
//! kept yet (seconds, or minutes while the servers are checked), and an
//! address could become both kinds of account when the two sign-ins
//! finish together. So:
//!
//! - Each address has a turn ([`GrantLedger::turn`]): keeping a token
//!   (after checking that the other kind holds none) and revoking one
//!   (after checking that the other kind does not share it) each take it.
//! - A sign-in whose grant may be Google's is counted from before its
//!   browser opens until its token is kept or given up
//!   ([`GrantLedger::under_way`]): the other kind does not revoke while
//!   one is, as its token may already share the grant. A token left
//!   unrevoked only lapses.
//!
//! The ledger lives with the keychain it describes: one per process for
//! the OS keychain ([`crate::imap::ImapSecrets::keychain`]), one per
//! in-memory keychain in tests. Another process (`postrinch-cli` beside
//! the window) has its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use postrinch_model::{EmailAddress, Provider};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Sign-ins under way and turns, by address key.
#[derive(Default)]
pub(crate) struct GrantLedger {
    addresses: Mutex<HashMap<String, Entry>>,
}

#[derive(Default)]
struct Entry {
    turn: Arc<AsyncMutex<()>>,
    google: usize,
    imap: usize,
}

impl Entry {
    fn count(&mut self, kind: Provider) -> &mut usize {
        match kind {
            Provider::Google => &mut self.google,
            Provider::Imap => &mut self.imap,
        }
    }

    /// Nothing under way and nobody holding or waiting for the turn: the
    /// entry can go.
    fn idle(&self) -> bool {
        self.google == 0 && self.imap == 0 && Arc::strong_count(&self.turn) == 1
    }
}

/// A sign-in counted as under way, until this is dropped.
pub(crate) struct UnderWay {
    ledger: Arc<GrantLedger>,
    key: String,
    kind: Provider,
}

impl Drop for UnderWay {
    fn drop(&mut self) {
        let mut addresses = self.ledger.lock();
        if let Some(entry) = addresses.get_mut(&self.key) {
            let count = entry.count(self.kind);
            *count = count.saturating_sub(1);
            if entry.idle() {
                addresses.remove(&self.key);
            }
        }
    }
}

/// The address's turn, held until this is dropped.
pub(crate) struct Turn {
    ledger: Arc<GrantLedger>,
    key: String,
    held: Option<OwnedMutexGuard<()>>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        // Let the lock go first, so the entry can be seen to be idle.
        self.held.take();
        let mut addresses = self.ledger.lock();
        if addresses.get(&self.key).is_some_and(Entry::idle) {
            addresses.remove(&self.key);
        }
    }
}

impl GrantLedger {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.addresses.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Counts a sign-in of `kind` for `address` as under way.
    pub(crate) fn under_way(self: &Arc<Self>, address: &EmailAddress, kind: Provider) -> UnderWay {
        let key = address.key();
        *self.lock().entry(key.clone()).or_default().count(kind) += 1;
        UnderWay {
            ledger: Arc::clone(self),
            key,
            kind,
        }
    }

    /// Whether a sign-in of `kind` is under way for `address`.
    pub(crate) fn is_under_way(&self, address: &EmailAddress, kind: Provider) -> bool {
        self.lock()
            .get_mut(&address.key())
            .is_some_and(|entry| *entry.count(kind) > 0)
    }

    /// Waits for `address`'s turn.
    pub(crate) async fn turn(self: &Arc<Self>, address: &EmailAddress) -> Turn {
        let key = address.key();
        let lock = Arc::clone(&self.lock().entry(key.clone()).or_default().turn);
        let held = lock.lock_owned().await;
        Turn {
            ledger: Arc::clone(self),
            key,
            held: Some(held),
        }
    }

    /// How many addresses have an entry (for tests: none once everything
    /// is done).
    #[cfg(test)]
    fn entries(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn ada() -> EmailAddress {
        EmailAddress::parse("Ada@Example.org").unwrap()
    }

    #[test]
    fn sign_ins_are_counted_by_kind_until_dropped() {
        let ledger = Arc::new(GrantLedger::default());
        let imap = ledger.under_way(&ada(), Provider::Imap);
        let lower = EmailAddress::parse("ada@example.org").unwrap();
        assert!(ledger.is_under_way(&lower, Provider::Imap));
        assert!(!ledger.is_under_way(&lower, Provider::Google));
        let again = ledger.under_way(&ada(), Provider::Imap);
        drop(imap);
        assert!(ledger.is_under_way(&ada(), Provider::Imap));
        drop(again);
        assert!(!ledger.is_under_way(&ada(), Provider::Imap));
        assert_eq!(ledger.entries(), 0);
    }

    #[tokio::test]
    async fn one_turn_at_a_time_per_address() {
        let ledger = Arc::new(GrantLedger::default());
        let first = ledger.turn(&ada()).await;
        // Another address is not held up.
        let bob = EmailAddress::parse("bob@example.org").unwrap();
        drop(ledger.turn(&bob).await);
        let waiting = {
            let ledger = Arc::clone(&ledger);
            tokio::spawn(async move { drop(ledger.turn(&ada()).await) })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "the second turn waited");
        drop(first);
        waiting.await.unwrap();
        assert_eq!(ledger.entries(), 0);
    }
}
