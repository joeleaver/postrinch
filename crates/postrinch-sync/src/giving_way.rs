//! Backfill giving way to passes, for both engines' keep-in-sync loops
//! (docs/imap.md, "Backfill gives way"; `google::sync::keep`).
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
//!
//! Backfill is never starved by the passes it gives way to. The poll
//! interval counts from when backfill began, not the pass, so a sync as
//! long as the interval still leaves backfill a whole interval. And a
//! backfill that gave way to a wake is owed its share next pass
//! ([`Share`]): a wake then stops it only once it has run as long as that
//! pass's sync took, so wakes that keep coming leave it at least half the
//! account's time, while a lone wake (Sync now) is answered at the next
//! batch.

use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::{Phase, Progress, ProgressFn, SyncError, SyncReport, SyncResult};

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

/// A backfill that failed partway: how many messages it went through
/// first, as [`Backfilled::through`], and what stopped it.
#[derive(Debug)]
pub(crate) struct Unfinished {
    pub(crate) through: usize,
    pub(crate) error: SyncError,
}

impl From<SyncError> for Unfinished {
    /// A failure before any message was gone through.
    fn from(error: SyncError) -> Self {
        Self { through: 0, error }
    }
}

/// What a pass owes its backfill, from how the last one's ended: after
/// one that gave way to a wake, a wake waits until backfill has run as
/// long as the pass's sync took.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Share {
    /// Whether the last pass's backfill took a wake.
    after_wake: bool,
}

impl Share {
    /// For a backfill beginning now, after a sync that took `synced_in`,
    /// woken by `wake`.
    pub(crate) fn give_way<'a>(&self, wake: &'a Notify, synced_in: Duration) -> GiveWay<'a> {
        GiveWay {
            wake,
            began: Instant::now(),
            owed: if self.after_wake {
                synced_in
            } else {
                Duration::ZERO
            },
            woken: false,
        }
    }

    /// Counts how a pass ended: whether its backfill took a wake. A pass
    /// that did not backfill (its sync failed) took none.
    pub(crate) fn passed(&mut self, woken: bool) {
        self.after_wake = woken;
    }
}

/// Whether a pass is due, for a backfill to give way to it: the loop was
/// woken, or the poll interval passed since the backfill began.
pub(crate) struct GiveWay<'a> {
    wake: &'a Notify,
    began: Instant,
    /// How long backfill runs before a wake can stop it ([`Share`]).
    owed: Duration,
    woken: bool,
}

impl GiveWay<'_> {
    /// Whether the next pass is due: `interval` gone by since backfill
    /// began, or woken once backfill has had what it is owed. A wake is
    /// taken only then: the pass the loop runs at once is the one it
    /// asked for, so it never brings a second; one that comes sooner is
    /// left for a later ask, or for the loop's wait. Once woken, it stays
    /// due.
    pub(crate) fn due(&mut self, interval: Duration) -> bool {
        let ran = self.began.elapsed();
        if self.woken || ran >= interval {
            return true;
        }
        // A Notify woken while nobody waited keeps one permit; a
        // notified() polled once takes it, or leaves none behind.
        if ran >= self.owed && self.wake.notified().now_or_never().is_some() {
            self.woken = true;
        }
        self.woken
    }

    /// Whether it took a wake. A loop runs its next pass at once when it
    /// did, however the backfill then ended (gave way, finished, failed):
    /// the wake is used up, so a wait would not end for it.
    pub(crate) fn woken(&self) -> bool {
        self.woken
    }
}

/// One account's backfill counted across the passes it gave way to, so
/// its progress reads as one download.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tally {
    /// Messages gone through by the backfills that gave way or failed
    /// since the last one that finished.
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

    /// Counts a backfill that ended, and gives its result as a sync's:
    /// one that gave way, or failed partway, adds what it went through to
    /// what is carried, as the next goes on from there; one that finished
    /// starts the count again.
    pub(crate) fn count(
        &mut self,
        backfilled: Result<Backfilled, Unfinished>,
    ) -> SyncResult<Backfilled> {
        match backfilled {
            Ok(backfilled) => {
                self.carried = if backfilled.gave_way {
                    self.carried + backfilled.through
                } else {
                    0
                };
                Ok(backfilled)
            }
            Err(Unfinished { through, error }) => {
                self.carried += through;
                Err(error)
            }
        }
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

    const HOUR: Duration = Duration::from_secs(3600);

    #[tokio::test]
    async fn a_wake_is_taken_once_and_stays_due() {
        let wake = Notify::new();
        let mut give_way = Share::default().give_way(&wake, HOUR);
        assert!(!give_way.due(HOUR));
        wake.notify_one();
        assert!(give_way.due(HOUR), "owed nothing: a wake stops it at once");
        assert!(give_way.woken());
        assert!(give_way.due(HOUR), "stays due");
        // The wake was taken: a waiter now waits for the next.
        assert!(wake.notified().now_or_never().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn the_interval_counts_from_when_backfill_began() {
        let wake = Notify::new();
        // A sync as long as the interval leaves backfill a whole one.
        let synced_in = Duration::from_secs(60);
        tokio::time::advance(synced_in).await;
        let mut give_way = Share::default().give_way(&wake, synced_in);
        assert!(!give_way.due(Duration::from_secs(60)));
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(!give_way.due(Duration::from_secs(60)));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(give_way.due(Duration::from_secs(60)));
        assert!(!give_way.woken());
    }

    #[tokio::test(start_paused = true)]
    async fn after_giving_way_to_a_wake_backfill_is_owed_its_sync_s_time() {
        let wake = Notify::new();
        let mut share = Share::default();
        wake.notify_one();
        let mut first = share.give_way(&wake, Duration::from_secs(5));
        assert!(first.due(HOUR));
        share.passed(first.woken());
        // The next pass's sync took 5 s; the wakes keep coming.
        wake.notify_one();
        let mut second = share.give_way(&wake, Duration::from_secs(5));
        assert!(!second.due(HOUR), "not before its share");
        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(!second.due(HOUR));
        assert!(!second.woken());
        // The wake was left, not taken, while backfill was owed time.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(second.due(HOUR));
        assert!(second.woken());
        share.passed(second.woken());
        // A pass that took no wake owes the next nothing.
        share.passed(false);
        wake.notify_one();
        assert!(share.give_way(&wake, HOUR).due(HOUR));
    }

    #[test]
    fn a_backfill_that_failed_partway_carries_its_count_on() {
        let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let log = seen.clone();
        let inner: ProgressFn = Arc::new(move |p| log.lock().unwrap().push(p));
        let bodies = |done, total| Progress::Working {
            phase: Phase::Bodies,
            done,
            total,
        };
        let mut tally = Tally::default();
        tally.progress(inner.clone())(bodies(9, 12));
        let failed = tally.count(Err(Unfinished {
            through: 9,
            error: SyncError::Io(std::io::Error::other("dropped")),
        }));
        assert!(matches!(failed, Err(SyncError::Io(_))));
        // Three left: the count goes on from nine, not from nothing.
        tally.progress(inner)(bodies(0, 3));
        assert_eq!(*seen.lock().unwrap(), [bodies(9, 12), bodies(9, 12)]);
    }

    fn ended(tally: &mut Tally, through: usize, gave_way: bool) {
        let backfilled = Backfilled {
            through,
            gave_way,
            ..Backfilled::default()
        };
        assert_eq!(tally.count(Ok(backfilled.clone())).unwrap(), backfilled);
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
        ended(&mut tally, 20, true);
        // 80 left, and a new message the pass between found.
        tally.progress(inner.clone())(bodies(0, 81));
        tally.progress(inner.clone())(Progress::Listing { found: 3 });
        ended(&mut tally, 81, false);
        tally.progress(inner.clone())(bodies(0, 5));
        ended(&mut tally, 5, true);
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
