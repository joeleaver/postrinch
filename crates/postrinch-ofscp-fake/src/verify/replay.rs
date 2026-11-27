//! The replay store (§4.5 step 4 and *Replay store*).
//!
//! One store covers every signed surface. Entries are keyed by (signer
//! kind, `key_id`, nonce) and kept until their timestamp plus the skew
//! window has passed. A pair is recorded only once the signature has
//! verified, re-checking atomically.
//!
//! A fake provider starts with no earlier incarnation, so no nonce it
//! could have seen is lost and there is no warm-up at start. A restart is
//! simulated with [`ReplayStore::restart`], after which the store refuses
//! every signature timestamped before T₀ + the skew window with `503`, as
//! the spec requires of a store that does not survive restarts.

use std::collections::HashMap;
use std::sync::Mutex;

use jiff::{SignedDuration, Timestamp};

use crate::problem::Problem;

/// The ±300 s window of §4.5 step 3.
pub(crate) const SKEW: SignedDuration = SignedDuration::from_secs(300);

/// Entries the store holds at most; when full it fails closed (`503`).
const CAPACITY: usize = 100_000;

/// Who signed: a user's device key (§4.4) or a provider (§8.1). The two
/// `key_id` spaces must not collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Signer {
    Actor,
}

#[derive(Debug, Default)]
struct Store {
    seen: HashMap<(Signer, String, String), Timestamp>,
    /// T₀ of the current store, set by a simulated restart.
    started: Option<Timestamp>,
}

#[derive(Debug, Default)]
pub(crate) struct ReplayStore {
    store: Mutex<Store>,
}

impl ReplayStore {
    /// Forget every nonce and start the warm-up from `now`.
    pub(crate) fn restart(&self, now: Timestamp) {
        let mut store = self.store.lock().unwrap();
        store.seen.clear();
        store.started = Some(now);
    }

    /// End the warm-up early (a test fault).
    pub(crate) fn end_warm_up(&self) {
        self.store.lock().unwrap().started = None;
    }

    /// When the warm-up ends, if `now` is still inside it.
    pub(crate) fn warm_up_until(&self, now: Timestamp) -> Option<Timestamp> {
        let started = self.store.lock().unwrap().started?;
        let until = started + SKEW;
        (now < until).then_some(until)
    }

    /// The warm-up rule: a signature timestamped before T₀ + skew is
    /// refused with `503` and a `Retry-After` of at least the time left.
    pub(crate) fn check_warm_up(
        &self,
        timestamp: Timestamp,
        now: Timestamp,
    ) -> Result<(), Problem> {
        let Some(started) = self.store.lock().unwrap().started else {
            return Ok(());
        };
        let until = started + SKEW;
        if timestamp >= until {
            return Ok(());
        }
        let left = until.duration_since(now).as_secs().max(1);
        Err(Problem::unavailable(
            "the replay store is warming up after a restart",
            Some(left.unsigned_abs()),
        ))
    }

    /// Step 4: has this pair been seen?
    pub(crate) fn seen(&self, signer: Signer, key_id: &str, nonce: &str) -> bool {
        self.store
            .lock()
            .unwrap()
            .seen
            .contains_key(&(signer, key_id.to_owned(), nonce.to_owned()))
    }

    /// After step 7: re-check and record the pair atomically. `Ok(false)`
    /// means a concurrent copy verified first (a replay).
    pub(crate) fn record(
        &self,
        signer: Signer,
        key_id: &str,
        nonce: &str,
        timestamp: Timestamp,
        now: Timestamp,
    ) -> Result<bool, Problem> {
        let mut store = self.store.lock().unwrap();
        let key = (signer, key_id.to_owned(), nonce.to_owned());
        if store.seen.contains_key(&key) {
            return Ok(false);
        }
        if store.seen.len() >= CAPACITY {
            store.seen.retain(|_, expires| *expires > now);
            if store.seen.len() >= CAPACITY {
                return Err(Problem::unavailable("the replay store is full", Some(1)));
            }
        }
        store.seen.insert(key, timestamp + SKEW);
        Ok(true)
    }
}
