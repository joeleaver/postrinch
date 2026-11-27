//! Each provider's clock, as this machine sees it.
//!
//! Providers refuse a signature more than 300 s off their own clock
//! (§4.5 step 3), and a desktop's clock can drift further than that. The
//! spec gives a client no clock endpoint, so the transport reads the
//! `Date` header (RFC 9110 §6.6.1) every provider response carries, keeps
//! the difference per provider, and signs with the provider's time.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::SystemTime;

use jiff::{SignedDuration, Timestamp};

/// How far apart a signature's time and the provider's may be (§4.5).
pub const SKEW_WINDOW_SECS: i64 = 300;

/// A `Date` header has whole seconds, and the response took some time to
/// arrive, so differences this small are noise and keep the offset as is.
const NOISE_SECS: i64 = 2;

/// How close to the window's edge a signature must have been for a `401`
/// to be put down to the clock and the request signed again.
const EDGE_MARGIN_SECS: i64 = 10;

/// One provider's clock: this machine's, corrected by the offset last seen
/// in the provider's `Date` header. Cloning shares the offset, so a
/// provider's REST client and its socket agree.
#[derive(Debug, Clone, Default)]
pub struct ProviderClock {
    /// The provider's time minus this machine's, in seconds.
    offset_secs: Arc<AtomicI64>,
}

impl ProviderClock {
    /// A clock that trusts this machine until the provider says otherwise.
    pub fn new() -> Self {
        Self::default()
    }

    /// The provider's time now, as far as is known.
    pub fn now(&self) -> Timestamp {
        let now = Timestamp::now();
        // An offset comes from a parsed HTTP date, so it is always in
        // range; were it not, this machine's time is the better guess.
        now.checked_add(SignedDuration::from_secs(self.offset_secs()))
            .unwrap_or(now)
    }

    /// The provider's time minus this machine's, in seconds.
    pub fn offset_secs(&self) -> i64 {
        self.offset_secs.load(Ordering::Relaxed)
    }

    /// Take note of a response's `Date` header. One that does not parse is
    /// ignored.
    pub fn observe_date(&self, date: &str) {
        let Ok(server) = httpdate::parse_http_date(date) else {
            tracing::debug!(date, "ignoring a Date header that does not parse");
            return;
        };
        self.observe(server, SystemTime::now());
    }

    /// Take note of a time the provider stated as its own now, such as a
    /// WebSocket frame's `ts` (§7.1).
    pub fn observe_time(&self, server: Timestamp) {
        self.observe(SystemTime::from(server), SystemTime::now());
    }

    fn observe(&self, server: SystemTime, local: SystemTime) {
        let measured = match server.duration_since(local) {
            Ok(ahead) => secs_i64(ahead.as_secs()),
            Err(behind) => -secs_i64(behind.duration().as_secs()),
        };
        let current = self.offset_secs();
        if (measured - current).abs() > NOISE_SECS {
            tracing::info!(
                from = current,
                to = measured,
                "provider clock offset changed"
            );
            self.offset_secs.store(measured, Ordering::Relaxed);
        }
    }

    /// Whether a request signed while the offset was `offset_at_signing`
    /// was at or near the edge of the provider's window, judged by how far
    /// the offset has moved since: the clock error alone, not how long the
    /// request took. After a `401` this says the clock, not the key, was
    /// the likely cause.
    pub fn explains_a_rejection(&self, offset_at_signing: i64) -> bool {
        let error = self.offset_secs().saturating_sub(offset_at_signing).abs();
        error >= SKEW_WINDOW_SECS - EDGE_MARGIN_SECS
    }
}

fn secs_i64(secs: u64) -> i64 {
    i64::try_from(secs).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_provider_ahead_moves_the_clock_forward() {
        let clock = ProviderClock::new();
        let local = SystemTime::now();
        clock.observe(local + Duration::from_secs(600), local);
        assert_eq!(clock.offset_secs(), 600);
        let ahead = clock.now().duration_since(Timestamp::now()).as_secs();
        assert!((599..=601).contains(&ahead), "{ahead}");
    }

    #[test]
    fn a_provider_behind_moves_it_back() {
        let clock = ProviderClock::new();
        let local = SystemTime::now();
        clock.observe(local - Duration::from_secs(400), local);
        assert_eq!(clock.offset_secs(), -400);
    }

    #[test]
    fn small_differences_are_noise() {
        let clock = ProviderClock::new();
        let local = SystemTime::now();
        clock.observe(local + Duration::from_secs(2), local);
        assert_eq!(clock.offset_secs(), 0);
        clock.observe(local - Duration::from_secs(1), local);
        assert_eq!(clock.offset_secs(), 0);
    }

    #[test]
    fn clones_share_the_offset() {
        let clock = ProviderClock::new();
        let other = clock.clone();
        let local = SystemTime::now();
        clock.observe(local + Duration::from_secs(100), local);
        assert_eq!(other.offset_secs(), 100);
    }

    #[test]
    fn a_date_header_is_read() {
        let clock = ProviderClock::new();
        let date = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(1000));
        clock.observe_date(&date);
        assert!((998..=1001).contains(&clock.offset_secs()));
        clock.observe_date("not a date");
        assert!((998..=1001).contains(&clock.offset_secs()));
    }

    #[test]
    fn only_a_moved_offset_explains_a_rejection() {
        let clock = ProviderClock::new();
        let at_signing = clock.offset_secs();
        assert!(!clock.explains_a_rejection(at_signing));
        let local = SystemTime::now();
        clock.observe(local + Duration::from_secs(301), local);
        assert!(clock.explains_a_rejection(at_signing));
        // Signed on the corrected clock: a slow request is not skew.
        assert!(!clock.explains_a_rejection(clock.offset_secs()));
        clock.observe(local - Duration::from_secs(5), local);
        assert!(clock.explains_a_rejection(301));
    }
}
