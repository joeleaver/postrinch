//! Waiting out a busy server ([`SyncError::Busy`](crate::SyncError::Busy))
//! in the IMAP keep-in-sync loops: one [`Backoff`] per account and server,
//! and the moment until which nothing more is asked of it.
//!
//! An account has one IMAP server and one SMTP server, so a server is
//! named by the account and the service (`"IMAP"`, `"SMTP"`). The
//! account's sync loop and its sending loop share each server's state:
//! whichever meets the server busy first takes the next wait from its
//! backoff, and a busy answer met during that wait (the other loop's
//! connection, turned away too) does not count again, so two loops never
//! double the wait between them. Both wait until the moment passes before
//! asking the server anything; a pass that signs in resets it.
//!
//! [`ImapSync`](super::ImapSync) holds the gate, so an account that is
//! removed, or added again (its servers checked, perhaps new ones), has
//! its waits forgotten ([`BusyGate::forget`]): it never waits out a
//! server it no longer has, nor a back-off from before it was removed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use postrinch_model::EmailAddress;
use postrinch_sync_core::Backoff;

/// One server's state: how long its waits have grown, and until when
/// nothing is to be asked of it.
#[derive(Debug, Default)]
struct Waiting {
    backoff: Backoff,
    until: Option<Instant>,
}

/// Every IMAP account's servers' busy state. Cheap to clone; clones
/// share it.
#[derive(Debug, Clone, Default)]
pub(crate) struct BusyGate {
    servers: Arc<Mutex<HashMap<(String, &'static str), Waiting>>>,
    /// The backoff each server starts with (tests make it short).
    policy: Option<(Duration, Duration)>,
}

impl BusyGate {
    /// Waits from `first`, doubling, up to `cap`, in place of
    /// [`Backoff::new`]'s.
    pub(crate) fn with_policy(first: Duration, cap: Duration) -> Self {
        Self {
            servers: Arc::default(),
            policy: Some((first, cap)),
        }
    }

    /// The server answered busy: how long to wait before asking it
    /// anything again. Within a wait already running, the rest of that
    /// wait; otherwise the backoff's next step, which starts now.
    pub(crate) fn busy(&self, account: &EmailAddress, service: &'static str) -> Duration {
        let now = Instant::now();
        let mut servers = self.lock();
        let waiting = servers
            .entry((account.key(), service))
            .or_insert_with(|| Waiting {
                backoff: self.backoff(),
                until: None,
            });
        if let Some(left) = waiting
            .until
            .and_then(|until| until.checked_duration_since(now))
            && !left.is_zero()
        {
            return left;
        }
        let wait = waiting.backoff.next_delay();
        waiting.until = Some(now + wait);
        wait
    }

    /// How long until the server may be asked again, if it is being
    /// waited out.
    pub(crate) fn remaining(
        &self,
        account: &EmailAddress,
        service: &'static str,
    ) -> Option<Duration> {
        let servers = self.lock();
        let until = servers.get(&(account.key(), service))?.until?;
        until
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
    }

    /// The server answered: the next busy answer waits the first step.
    pub(crate) fn reset(&self, account: &EmailAddress, service: &'static str) {
        self.lock().remove(&(account.key(), service));
    }

    /// The account is gone, or its servers were just checked: forgets
    /// both its servers' waits.
    pub(crate) fn forget(&self, account: &EmailAddress) {
        let key = account.key();
        self.lock().retain(|(account, _), _| *account != key);
    }

    fn backoff(&self) -> Backoff {
        match self.policy {
            Some((first, cap)) => Backoff::with(first, cap),
            None => Backoff::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, &'static str), Waiting>> {
        self.servers.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ada() -> EmailAddress {
        EmailAddress::parse("ada@example.org").unwrap()
    }

    #[test]
    fn waits_grow_and_reset() {
        let gate = BusyGate::with_policy(Duration::from_millis(40), Duration::from_secs(10));
        let first = gate.busy(&ada(), "IMAP");
        assert!(first >= Duration::from_millis(20) && first <= Duration::from_millis(40));
        assert!(gate.remaining(&ada(), "IMAP").is_some());
        // Another server of the account, and another account, are not
        // held up.
        assert_eq!(gate.remaining(&ada(), "SMTP"), None);
        let bob = EmailAddress::parse("bob@example.org").unwrap();
        assert_eq!(gate.remaining(&bob, "IMAP"), None);

        std::thread::sleep(first + Duration::from_millis(5));
        assert_eq!(gate.remaining(&ada(), "IMAP"), None);
        let second = gate.busy(&ada(), "IMAP");
        assert!(second >= Duration::from_millis(40), "{second:?}");

        gate.reset(&ada(), "IMAP");
        assert_eq!(gate.remaining(&ada(), "IMAP"), None);
        let again = gate.busy(&ada(), "IMAP");
        assert!(again <= Duration::from_millis(40), "{again:?}");
    }

    #[test]
    fn forgetting_an_account_ends_both_its_waits_and_no_one_elses() {
        let gate = BusyGate::with_policy(Duration::from_secs(30), Duration::from_secs(900));
        let bob = EmailAddress::parse("bob@example.org").unwrap();
        for service in ["IMAP", "SMTP"] {
            gate.busy(&ada(), service);
            gate.busy(&ada(), service);
            gate.busy(&bob, service);
        }
        gate.forget(&EmailAddress::parse("Ada@Example.org").unwrap());
        assert_eq!(gate.remaining(&ada(), "IMAP"), None);
        assert_eq!(gate.remaining(&ada(), "SMTP"), None);
        assert!(gate.remaining(&bob, "IMAP").is_some());
        assert!(gate.remaining(&bob, "SMTP").is_some());
        // The back-off starts again from its first step.
        let again = gate.busy(&ada(), "IMAP");
        assert!(again <= Duration::from_secs(30), "{again:?}");
    }

    #[test]
    fn a_busy_answer_within_a_wait_does_not_count_again() {
        let gate = BusyGate::with_policy(Duration::from_secs(30), Duration::from_secs(900));
        let first = gate.busy(&ada(), "IMAP");
        let second = gate.busy(&ada(), "IMAP");
        assert!(second <= first);
        // Still the first step's wait: a third busy answer after it would
        // be the second step, not the third.
        std::thread::sleep(Duration::from_millis(1));
        let third = gate.busy(&ada(), "IMAP");
        assert!(third <= Duration::from_secs(30), "{third:?}");
    }
}
