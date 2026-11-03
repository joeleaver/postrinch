//! Pacing Gmail requests to stay inside Gmail's per-user quota.
//!
//! Gmail allows each user 250 quota units a second, and every method has a
//! cost (a message fetch is 5). Sync runs several fetches at once, so
//! waiting to be told "rate limited" and backing off one request at a time
//! just has every worker hit the limit in turn. Instead each account has a
//! token bucket that requests draw their cost from; when Gmail still says
//! the account is over its limit, the bucket is emptied below zero, which
//! pauses every request for that account together.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Units a second, under Gmail's 250 to leave room for other clients of the
/// same account (the person's phone, Gmail itself).
const RATE: f64 = 200.0;
/// The most units that can be spent at once after an idle spell.
const BURST: f64 = 200.0;

/// Gmail's documented quota cost of each method used.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Cost {
    GetProfile,
    ListLabels,
    ListHistory,
    ListMessages,
    GetMessage,
}

impl Cost {
    fn units(self) -> f64 {
        match self {
            Cost::GetProfile | Cost::ListLabels => 1.0,
            Cost::ListHistory => 2.0,
            Cost::ListMessages | Cost::GetMessage => 5.0,
        }
    }
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl Bucket {
    fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.updated).as_secs_f64();
        self.tokens = (self.tokens + elapsed * RATE).min(BURST);
        self.updated = now;
    }
}

#[derive(Default)]
pub(crate) struct Quota {
    buckets: Mutex<HashMap<String, Arc<Mutex<Bucket>>>>,
}

impl Quota {
    async fn bucket(&self, account: &str) -> Arc<Mutex<Bucket>> {
        let mut buckets = self.buckets.lock().await;
        Arc::clone(buckets.entry(account.to_owned()).or_insert_with(|| {
            Arc::new(Mutex::new(Bucket {
                tokens: BURST,
                updated: Instant::now(),
            }))
        }))
    }

    /// Wait until `account` can spend `cost`, then spend it.
    pub(crate) async fn acquire(&self, account: &str, cost: Cost) {
        let cost = cost.units();
        let bucket = self.bucket(account).await;
        loop {
            let wait = {
                let mut b = bucket.lock().await;
                b.refill(Instant::now());
                if b.tokens >= cost {
                    b.tokens -= cost;
                    return;
                }
                Duration::from_secs_f64((cost - b.tokens) / RATE)
            };
            tokio::time::sleep(wait).await;
        }
    }

    /// Gmail said the account is over its limit: hold every request for it
    /// for at least `pause` from now. Pauses do not add up: when several
    /// requests in flight are told the same thing, the account waits for the
    /// longest of them, not their sum.
    pub(crate) async fn back_off(&self, account: &str, pause: Duration) {
        let bucket = self.bucket(account).await;
        let mut b = bucket.lock().await;
        b.refill(Instant::now());
        b.tokens = b.tokens.min(-pause.as_secs_f64() * RATE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn paces_to_the_rate_after_the_burst() {
        let quota = Quota::default();
        let start = Instant::now();
        // The burst covers 40 fetches at once; the next 40 take a second.
        for _ in 0..80 {
            quota.acquire("a", Cost::GetMessage).await;
        }
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(990), "{took:?}");
        assert!(took <= Duration::from_millis(1100), "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn accounts_have_their_own_buckets() {
        let quota = Quota::default();
        let start = Instant::now();
        for _ in 0..40 {
            quota.acquire("a", Cost::GetMessage).await;
            quota.acquire("b", Cost::GetMessage).await;
        }
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn back_offs_do_not_add_up() {
        let quota = Quota::default();
        for _ in 0..10 {
            quota.back_off("a", Duration::from_secs(2)).await;
        }
        let start = Instant::now();
        quota.acquire("a", Cost::GetProfile).await;
        let took = start.elapsed();
        assert!(took >= Duration::from_secs(2), "{took:?}");
        assert!(took < Duration::from_millis(2100), "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn back_off_pauses_everyone() {
        let quota = Quota::default();
        quota.back_off("a", Duration::from_secs(2)).await;
        let start = Instant::now();
        quota.acquire("a", Cost::GetProfile).await;
        assert!(start.elapsed() >= Duration::from_secs(2));
    }
}
