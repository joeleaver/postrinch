//! The provider's clock: the real clock plus an offset a test can change,
//! so clock skew (§4.5 step 3) can be exercised from either side.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime};

use jiff::{SignedDuration, Timestamp};

/// The real time shifted by a settable number of milliseconds.
#[derive(Debug, Default)]
pub(crate) struct Clock {
    offset_ms: AtomicI64,
}

impl Clock {
    /// Shift this provider's clock by `offset` from the real time
    /// (positive: the provider runs ahead).
    pub(crate) fn set_offset(&self, offset: SignedDuration) {
        let ms = i64::try_from(offset.as_millis()).unwrap_or(i64::MAX);
        self.offset_ms.store(ms, Ordering::SeqCst);
    }

    pub(crate) fn offset(&self) -> SignedDuration {
        SignedDuration::from_millis(self.offset_ms.load(Ordering::SeqCst))
    }

    /// Now, on this provider's clock.
    pub(crate) fn now(&self) -> Timestamp {
        Timestamp::now()
            .checked_add(self.offset())
            .expect("a test clock offset stays within jiff's range")
    }

    /// Now, as a `SystemTime`, for the HTTP `Date` header.
    pub(crate) fn system_now(&self) -> SystemTime {
        let offset = self.offset();
        let magnitude = Duration::from_millis(offset.as_millis().unsigned_abs() as u64);
        if offset.is_negative() {
            SystemTime::now() - magnitude
        } else {
            SystemTime::now() + magnitude
        }
    }
}

/// `t` truncated to whole seconds, as OFSCP timestamps are written (§2.5).
pub(crate) fn whole_seconds(t: Timestamp) -> Timestamp {
    Timestamp::from_second(t.as_second()).expect("a valid timestamp truncates to a valid one")
}
