//! Pacing Gmail requests to stay inside Gmail's per-user quota.
//!
//! Gmail allows each user of a project 6,000 quota units a minute, and every
//! method has a cost: a message fetched whole (`format=raw`) costs 60, so an
//! account can download about 100 messages a minute, whatever the client
//! does. Sync runs several requests at once, so waiting to be told "rate
//! limited" and backing off one request at a time just has every worker hit
//! the limit in turn. Instead each account has a token bucket that requests
//! draw their cost from.
//!
//! The person's other clients (their phone, Gmail itself) share the same
//! quota, so the rate also adapts: it halves when Gmail says "too fast"
//! anyway, pausing every request for the account together, and creeps back
//! up while Gmail stays quiet.
//!
//! Requests come in two priorities. Foreground ones (what the person is
//! waiting for: a sync, a message they opened) take what is there.
//! Background ones (backfilling the bodies of old mail) leave a reserve in
//! the bucket, so foreground work never waits behind them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Gmail's per-user limit, 6,000 units a minute, as units a second.
const GMAIL_RATE: f64 = 100.0;
/// The starting rate: a little under Gmail's, for the person's other clients.
const START_RATE: f64 = 90.0;
/// The rate never goes above this, however quiet Gmail is.
const MAX_RATE: f64 = GMAIL_RATE * 0.95;
/// Nor below this, however often it complains.
const MIN_RATE: f64 = 10.0;
/// How fast the rate recovers while Gmail does not complain, in units a
/// second, per second.
const RECOVERY: f64 = 1.0;
/// The most units that can be spent at once after an idle spell: five
/// whole messages.
const BURST: f64 = 300.0;
/// What background requests leave in the bucket for foreground ones.
const RESERVE: f64 = 120.0;

/// Gmail's documented quota cost of each method used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cost {
    GetProfile,
    ListLabels,
    ListHistory,
    ListMessages,
    /// `messages.get` with `format=metadata` or `minimal`.
    GetMessageMetadata,
    /// `messages.get` with `format=raw` (or `full`).
    GetMessageRaw,
}

impl Cost {
    fn units(self) -> f64 {
        match self {
            Cost::GetProfile | Cost::ListLabels => 1.0,
            Cost::ListHistory => 2.0,
            Cost::ListMessages => 5.0,
            Cost::GetMessageMetadata => 20.0,
            Cost::GetMessageRaw => 60.0,
        }
    }
}

/// Whether a request is one the person is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    Foreground,
    Background,
}

struct Bucket {
    tokens: f64,
    /// Units a second, adjusted by what Gmail says.
    rate: f64,
    updated: Instant,
    /// Until when a rate-limit pause holds; the rate does not recover
    /// during it.
    paused_until: Instant,
}

impl Bucket {
    fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.updated).as_secs_f64();
        let calm = now.saturating_duration_since(self.paused_until.max(self.updated));
        self.rate = (self.rate + calm.as_secs_f64() * RECOVERY).min(MAX_RATE);
        self.tokens = (self.tokens + elapsed * self.rate).min(BURST);
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
            let now = Instant::now();
            Arc::new(Mutex::new(Bucket {
                tokens: BURST,
                rate: START_RATE,
                updated: now,
                paused_until: now,
            }))
        }))
    }

    /// Wait until `account` can spend `cost` at `priority`, then spend it.
    ///
    /// Foreground requests are served in the order they ask: each books its
    /// units at once, taking the balance below zero if it must, and waits
    /// until the balance is back to zero. So a 60-unit whole message is
    /// never starved by a stream of 20-unit header fetches that would each
    /// fit sooner. Background requests wait until the balance covers their
    /// cost with [`RESERVE`] to spare, and book nothing until it does: they
    /// only ever use what foreground work leaves.
    pub(crate) async fn acquire(&self, account: &str, cost: Cost, priority: Priority) {
        let cost = cost.units();
        let bucket = self.bucket(account).await;
        if priority == Priority::Foreground {
            let wait = {
                let mut b = bucket.lock().await;
                b.refill(Instant::now());
                b.tokens -= cost;
                if b.tokens >= 0.0 {
                    return;
                }
                Duration::from_secs_f64(-b.tokens / b.rate)
            };
            tokio::time::sleep(wait).await;
            return;
        }
        let needed = cost + RESERVE;
        loop {
            let wait = {
                let mut b = bucket.lock().await;
                b.refill(Instant::now());
                if b.tokens >= needed {
                    b.tokens -= cost;
                    return;
                }
                Duration::from_secs_f64((needed - b.tokens) / b.rate)
            };
            tokio::time::sleep(wait).await;
        }
    }

    /// Gmail said the account is over its limit: hold every request for it
    /// for at least `pause` from now, and go on at half the rate. Several
    /// requests in flight told the same thing count once: the account waits
    /// for the longest pause, and the rate halves once per pause.
    pub(crate) async fn back_off(&self, account: &str, pause: Duration) {
        let bucket = self.bucket(account).await;
        let mut b = bucket.lock().await;
        let now = Instant::now();
        b.refill(now);
        if now >= b.paused_until {
            b.rate = (b.rate / 2.0).max(MIN_RATE);
            tracing::debug!("{account}: rate limited; now {:.0} units/s", b.rate);
        }
        b.paused_until = b.paused_until.max(now + pause);
        b.tokens = b.tokens.min(-pause.as_secs_f64() * b.rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FG: Priority = Priority::Foreground;

    #[tokio::test(start_paused = true)]
    async fn paces_whole_messages_to_gmails_limit() {
        let quota = Quota::default();
        let start = Instant::now();
        // The burst covers 5 whole messages; at 90 units/s the next 15 take
        // 900 units, 10 seconds.
        for _ in 0..20 {
            quota.acquire("a", Cost::GetMessageRaw, FG).await;
        }
        let took = start.elapsed();
        assert!(took >= Duration::from_secs(9), "{took:?}");
        assert!(took <= Duration::from_secs(11), "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_whole_message_is_not_starved_by_cheaper_requests() {
        let quota = Arc::new(Quota::default());
        // Spend the burst, then keep ten header fetches always waiting, as
        // a full sync does.
        quota.acquire("a", Cost::GetMessageRaw, FG).await;
        for _ in 0..12 {
            quota.acquire("a", Cost::GetMessageMetadata, FG).await;
        }
        let stream = {
            let quota = Arc::clone(&quota);
            tokio::spawn(async move {
                let mut fetches = tokio::task::JoinSet::new();
                for _ in 0..200 {
                    while fetches.len() >= 10 {
                        fetches.join_next().await;
                    }
                    let quota = Arc::clone(&quota);
                    fetches.spawn(async move {
                        quota.acquire("a", Cost::GetMessageMetadata, FG).await;
                    });
                }
            })
        };
        tokio::task::yield_now().await;
        let start = Instant::now();
        quota.acquire("a", Cost::GetMessageRaw, FG).await;
        // Behind at most the ten already waiting: 260 units, under 3 s.
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
        stream.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn accounts_have_their_own_buckets() {
        let quota = Quota::default();
        let start = Instant::now();
        for _ in 0..5 {
            quota.acquire("a", Cost::GetMessageRaw, FG).await;
            quota.acquire("b", Cost::GetMessageRaw, FG).await;
        }
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn background_leaves_a_reserve() {
        let quota = Quota::default();
        // 300 in the bucket: background may spend down to the 120 reserve.
        for _ in 0..3 {
            quota
                .acquire("a", Cost::GetMessageRaw, Priority::Background)
                .await;
        }
        let start = Instant::now();
        quota.acquire("a", Cost::GetMessageRaw, FG).await;
        quota.acquire("a", Cost::GetMessageRaw, FG).await;
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "foreground found its reserve"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn back_offs_do_not_add_up() {
        let quota = Quota::default();
        for _ in 0..10 {
            quota.back_off("a", Duration::from_secs(2)).await;
        }
        let start = Instant::now();
        quota.acquire("a", Cost::GetProfile, FG).await;
        let took = start.elapsed();
        assert!(took >= Duration::from_secs(2), "{took:?}");
        assert!(took < Duration::from_millis(2200), "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn rate_halves_once_per_pause_and_recovers() {
        let quota = Quota::default();
        let rate = |q: &Quota| {
            let q = q.buckets.try_lock().unwrap();
            q["a"].try_lock().unwrap().rate
        };
        quota.acquire("a", Cost::GetProfile, FG).await;
        for _ in 0..5 {
            quota.back_off("a", Duration::from_secs(1)).await;
        }
        assert_eq!(rate(&quota), START_RATE / 2.0);

        // Calm for ten seconds after the pause: +1 unit/s each second.
        tokio::time::sleep(Duration::from_secs(11)).await;
        quota.acquire("a", Cost::GetProfile, FG).await;
        assert!((rate(&quota) - (START_RATE / 2.0 + 10.0)).abs() < 0.5);
    }
}
