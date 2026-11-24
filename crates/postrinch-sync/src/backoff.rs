//! How long to wait before trying again a server that said it is busy
//! ([`SyncError::Busy`](crate::SyncError::Busy)): exponential, with
//! jitter, capped, and back to the start once a try succeeds.
//!
//! The numbers ([`Backoff::FIRST`], [`Backoff::CAP`]) are for the limits
//! mail servers actually put on an account:
//!
//! - **Too many connections** (Gmail's "Too many simultaneous
//!   connections", Dovecot's `mail_max_userip_connections`, `[INUSE]`)
//!   clears as soon as another client's connection drops, often within
//!   a minute. So the first wait is short: about 30 seconds.
//! - **Bandwidth and command limits** (Gmail's "Account exceeded command
//!   or bandwidth limits", `[LIMIT]`) can last hours: Google's limits are
//!   per day and its suspension lifts when the period resets. Doubling
//!   from 30 seconds reaches the cap in five tries (about 15 minutes in),
//!   and from then on a try every 15 minutes costs one refused sign-in,
//!   no more than 96 a day, while noticing within 15 minutes that the
//!   limit lifted. Trying more often would add to the very count the
//!   server is limiting.
//!
//! The jitter ("equal jitter": each wait is between half and the whole of
//! its step) keeps the clients of several accounts on one server, or one
//! account's IMAP and SMTP, from all knocking at the same moment.

use std::time::Duration;

/// The waits after one failure after another, until a success resets
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    first: Duration,
    cap: Duration,
    failures: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

impl Backoff {
    /// The first step: a wait of 15 to 30 seconds after the first failure.
    pub const FIRST: Duration = Duration::from_secs(30);
    /// The longest wait.
    pub const CAP: Duration = Duration::from_secs(15 * 60);

    /// The policy for busy mail servers: from [`Self::FIRST`], doubling,
    /// up to [`Self::CAP`].
    pub fn new() -> Self {
        Self::with(Self::FIRST, Self::CAP)
    }

    /// A policy with other numbers (for tests, or another kind of limit).
    /// `cap` below `first` is raised to it.
    pub fn with(first: Duration, cap: Duration) -> Self {
        Self {
            first,
            cap: cap.max(first),
            failures: 0,
        }
    }

    /// How many failures in a row have been counted since the last reset.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Counts a failure and says how long to wait before the next try.
    pub fn next_delay(&mut self) -> Duration {
        let mut bytes = [0u8; 4];
        rand::fill(&mut bytes[..]);
        let unit = f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0);
        self.next_delay_with(unit)
    }

    /// As [`Self::next_delay`], with the jitter given: `unit` in `[0, 1)`
    /// picks the wait between half the step (0) and the whole step (1).
    fn next_delay_with(&mut self, unit: f64) -> Duration {
        let step = self.step();
        self.failures = self.failures.saturating_add(1);
        let half = step / 2;
        half + (step - half).mul_f64(unit.clamp(0.0, 1.0))
    }

    /// The current failure's step: `first` doubled once per earlier
    /// failure, at most `cap`.
    fn step(&self) -> Duration {
        // 2^20 times any sensible `first` is past any sensible cap.
        let doublings = self.failures.min(20);
        self.first.saturating_mul(1 << doublings).min(self.cap)
    }

    /// A try succeeded: the next failure waits the first step again.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn the_whole_step_doubles_up_to_the_cap() {
        let mut backoff = Backoff::new();
        let waits: Vec<u64> = (0..8)
            .map(|_| backoff.next_delay_with(1.0).as_secs())
            .collect();
        assert_eq!(waits, [30, 60, 120, 240, 480, 900, 900, 900]);
        assert_eq!(backoff.failures(), 8);
    }

    #[test]
    fn jitter_stays_between_half_and_the_whole_step() {
        let mut backoff = Backoff::new();
        assert_eq!(backoff.next_delay_with(0.0), 15 * SECOND);
        assert_eq!(backoff.next_delay_with(0.5), 45 * SECOND);
        for _ in 0..10 {
            backoff.next_delay_with(0.0);
        }
        assert_eq!(backoff.next_delay_with(0.0), Backoff::CAP / 2);
        // Out of range is clamped, not trusted.
        assert_eq!(backoff.next_delay_with(7.0), Backoff::CAP);
        assert_eq!(backoff.next_delay_with(-1.0), Backoff::CAP / 2);
    }

    #[test]
    fn random_waits_keep_within_their_bounds() {
        let mut backoff = Backoff::new();
        for failure in 0..12u32 {
            let step = (Backoff::FIRST * (1 << failure.min(10))).min(Backoff::CAP);
            let wait = backoff.next_delay();
            assert!(wait >= step / 2 && wait <= step, "{failure}: {wait:?}");
        }
    }

    #[test]
    fn reset_starts_again_from_the_first_step() {
        let mut backoff = Backoff::new();
        for _ in 0..4 {
            backoff.next_delay_with(1.0);
        }
        backoff.reset();
        assert_eq!(backoff.failures(), 0);
        assert_eq!(backoff.next_delay_with(1.0), Backoff::FIRST);
    }

    #[test]
    fn a_cap_below_the_first_step_is_the_first_step() {
        let mut backoff = Backoff::with(10 * SECOND, SECOND);
        assert_eq!(backoff.next_delay_with(1.0), 10 * SECOND);
        assert_eq!(backoff.next_delay_with(1.0), 10 * SECOND);
    }

    #[test]
    fn many_failures_do_not_overflow() {
        let mut backoff = Backoff::new();
        for _ in 0..10_000 {
            assert!(backoff.next_delay() <= Backoff::CAP);
        }
    }
}
