//! Backfill giving way to passes, for both engines' keep-in-sync loops
//! (docs/imap.md, "Sync"; `google::sync::keep`).
//!
//! A pass syncs, then downloads the bodies its sync left (backfill). On
//! a new account backfill takes most of an hour, and a pass that waited
//! for it would leave everything that asks for a pass waiting too: a
//! wake (mail sent, Sync now, an IDLE's news) and the poll interval. So
//! backfill stops between batches once a pass is due ([`GiveWay`]), the
//! loop runs that pass at once, and backfill carries on after its sync
//! from the newest message still left. The sidebar's count carries on
//! with it ([`Tally`]): "Downloading older mail" goes on from where it
//! was, not from nothing.

use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::{Phase, Progress, ProgressFn, SyncReport};

/// How a backfill ended.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Backfilled {
    pub(crate) report: SyncReport,
    /// How many of the messages it set out to download it went through,
    /// stored or not.
    pub(crate) through: usize,
    /// Whether it stopped for a pass with messages still left.
    pub(crate) gave_way: bool,
}

/// Whether a pass is due, for a backfill to give way to it: the loop was
/// woken, or the poll interval passed since the pass began.
pub(crate) struct GiveWay<'a> {
    wake: &'a Notify,
    began: Instant,
    woken: bool,
}

impl<'a> GiveWay<'a> {
    /// For a pass beginning now, woken by `wake`.
    pub(crate) fn new(wake: &'a Notify) -> Self {
        Self {
            wake,
            began: Instant::now(),
            woken: false,
        }
    }

    /// Whether the next pass is due: woken since the last pass began, or
    /// `interval` gone by since this one did. A wake is taken when it is
    /// seen: the pass the loop runs at once is the one it asked for, so
    /// it never brings a second. Once due, it stays due.
    pub(crate) fn due(&mut self, interval: Duration) -> bool {
        // A Notify woken while nobody waited keeps one permit; a
        // notified() polled once takes it, or leaves none behind.
        if !self.woken && self.wake.notified().now_or_never().is_some() {
            self.woken = true;
        }
        self.woken || self.began.elapsed() >= interval
    }

    /// Whether it was woken, rather than only the interval passing.
    pub(crate) fn woken(&self) -> bool {
        self.woken
    }
}

/// One account's backfill counted across the passes it gave way to, so
/// its progress reads as one download.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tally {
    /// Messages gone through by the backfills that gave way since the
    /// last one that finished.
    carried: usize,
}

impl Tally {
    /// `progress`, with what earlier backfills went through added to the
    /// bodies' count and its total: the total is what was left then and
    /// what the passes between added.
    pub(crate) fn progress(&self, progress: ProgressFn) -> ProgressFn {
        let carried = self.carried;
        if carried == 0 {
            return progress;
        }
        Arc::new(move |p| {
            progress(match p {
                Progress::Working {
                    phase: Phase::Bodies,
                    done,
                    total,
                } => Progress::Working {
                    phase: Phase::Bodies,
                    done: done + carried,
                    total: total + carried,
                },
                other => other,
            })
        })
    }

    /// Counts a backfill that ended: one that gave way adds to what is
    /// carried; one that finished starts the count again.
    pub(crate) fn ended(&mut self, backfilled: &Backfilled) {
        self.carried = if backfilled.gave_way {
            self.carried + backfilled.through
        } else {
            0
        };
    }

    /// Counts a pass's sync: one that listed the account, or a folder of
    /// it, whole (the account added again, a folder's UIDVALIDITY new)
    /// starts the count again, as what is left is listed afresh. A failed
    /// pass leaves the count be: what was gone through still was.
    pub(crate) fn synced(&mut self, report: &SyncReport) {
        if report.full {
            self.carried = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[tokio::test]
    async fn a_wake_is_taken_once_and_stays_due() {
        let wake = Notify::new();
        let mut give_way = GiveWay::new(&wake);
        assert!(!give_way.due(Duration::from_secs(3600)));
        wake.notify_one();
        assert!(give_way.due(Duration::from_secs(3600)));
        assert!(give_way.woken());
        assert!(give_way.due(Duration::from_secs(3600)), "stays due");
        // The wake was taken: a waiter now waits for the next.
        assert!(wake.notified().now_or_never().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn the_interval_passing_makes_a_pass_due_without_a_wake() {
        let wake = Notify::new();
        let mut give_way = GiveWay::new(&wake);
        assert!(!give_way.due(Duration::from_secs(60)));
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(give_way.due(Duration::from_secs(60)));
        assert!(!give_way.woken());
    }

    #[test]
    fn the_count_carries_on_across_backfills_that_gave_way() {
        let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let log = seen.clone();
        let inner: ProgressFn = Arc::new(move |p| log.lock().unwrap().push(p));
        let bodies = |done, total| Progress::Working {
            phase: Phase::Bodies,
            done,
            total,
        };
        let mut tally = Tally::default();
        tally.progress(inner.clone())(bodies(20, 100));
        tally.ended(&Backfilled {
            through: 20,
            gave_way: true,
            ..Backfilled::default()
        });
        // 80 left, and a new message the pass between found.
        tally.progress(inner.clone())(bodies(0, 81));
        tally.progress(inner.clone())(Progress::Listing { found: 3 });
        tally.ended(&Backfilled {
            through: 81,
            ..Backfilled::default()
        });
        tally.progress(inner.clone())(bodies(0, 5));
        tally.ended(&Backfilled {
            through: 5,
            gave_way: true,
            ..Backfilled::default()
        });
        tally.synced(&SyncReport {
            full: true,
            ..SyncReport::default()
        });
        tally.progress(inner)(bodies(0, 7));
        assert_eq!(
            *seen.lock().unwrap(),
            [
                bodies(20, 100),
                bodies(20, 101),
                Progress::Listing { found: 3 },
                bodies(0, 5),
                bodies(0, 7),
            ]
        );
    }
}
