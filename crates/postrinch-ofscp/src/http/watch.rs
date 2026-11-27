//! Watching an upload. A file takes as long as it takes to send, so an
//! upload has no overall timeout; instead it fails when its bytes stop
//! moving, or when the provider does not answer once they are all sent.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::time::{Instant, MissedTickBehavior};

use crate::{Error, Result};

/// How often the count of sent bytes is looked at.
const TICK: Duration = Duration::from_millis(500);

/// What the watch saw go wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stall {
    /// No byte was sent for the stall limit.
    Body,
    /// Every byte was sent, and no answer came within the answer limit.
    Answer,
}

/// The state of one attempt's watch.
#[derive(Debug)]
pub(crate) struct Watch {
    total: u64,
    stall: Duration,
    answer: Duration,
    /// Extra time before the first byte: connecting comes first.
    connect: Duration,
    seen: u64,
    moved_at: Instant,
    done_at: Option<Instant>,
}

impl Watch {
    pub fn new(
        total: u64,
        stall: Duration,
        answer: Duration,
        connect: Duration,
        now: Instant,
    ) -> Self {
        Self {
            total,
            stall,
            answer,
            connect,
            seen: 0,
            moved_at: now,
            done_at: None,
        }
    }

    /// Look at the count of bytes sent so far, at `now`.
    pub fn check(&mut self, sent: u64, now: Instant) -> Option<Stall> {
        if sent != self.seen {
            self.seen = sent;
            self.moved_at = now;
        }
        if sent >= self.total {
            let done_at = *self.done_at.get_or_insert(now);
            return (now.duration_since(done_at) >= self.answer).then_some(Stall::Answer);
        }
        let limit = if self.seen == 0 {
            self.connect + self.stall
        } else {
            self.stall
        };
        (now.duration_since(self.moved_at) >= limit).then_some(Stall::Body)
    }
}

/// Send an upload's `request`, whose body counts its bytes into `sent`,
/// and wait for the answer's headers under the watch's limits.
pub(crate) async fn send_upload(
    request: reqwest::RequestBuilder,
    sent: Arc<AtomicU64>,
    mut watch: Watch,
    authority: &str,
) -> Result<reqwest::Response> {
    let send = request.send();
    tokio::pin!(send);
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            response = &mut send => {
                return response.map_err(|source| Error::Network {
                    authority: authority.to_owned(),
                    source,
                });
            }
            _ = tick.tick() => {
                let stall = watch.check(sent.load(Ordering::Relaxed), Instant::now());
                if let Some(stall) = stall {
                    let (waiting_for, after) = match stall {
                        Stall::Body if watch.seen == 0 => {
                            ("the upload to start", watch.connect + watch.stall)
                        }
                        Stall::Body => ("the upload to move", watch.stall),
                        Stall::Answer => ("the answer to the upload", watch.answer),
                    };
                    return Err(Error::Stalled {
                        authority: authority.to_owned(),
                        waiting_for,
                        after,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    #[test]
    fn a_slow_but_moving_upload_is_not_a_stall() {
        let start = Instant::now();
        let mut watch = Watch::new(100, 30 * S, 30 * S, 10 * S, start);
        for i in 1..100 {
            assert_eq!(watch.check(i, start + i as u32 * 20 * S), None);
        }
    }

    #[test]
    fn an_upload_that_stops_moving_stalls() {
        let start = Instant::now();
        let mut watch = Watch::new(100, 30 * S, 60 * S, 10 * S, start);
        assert_eq!(watch.check(10, start + 5 * S), None);
        assert_eq!(watch.check(10, start + 34 * S), None);
        assert_eq!(watch.check(10, start + 35 * S), Some(Stall::Body));
    }

    #[test]
    fn connecting_gets_its_own_time_before_the_first_byte() {
        let start = Instant::now();
        let mut watch = Watch::new(100, 30 * S, 60 * S, 10 * S, start);
        assert_eq!(watch.check(0, start + 39 * S), None);
        assert_eq!(watch.check(0, start + 40 * S), Some(Stall::Body));
    }

    #[test]
    fn a_sent_upload_waits_for_its_answer() {
        let start = Instant::now();
        let mut watch = Watch::new(100, 30 * S, 60 * S, 10 * S, start);
        assert_eq!(watch.check(100, start + 10 * S), None);
        assert_eq!(watch.check(100, start + 69 * S), None);
        assert_eq!(watch.check(100, start + 70 * S), Some(Stall::Answer));
    }
}
