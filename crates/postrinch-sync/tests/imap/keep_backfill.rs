//! Backfill giving way to passes (docs/imap.md, "Sync"), through
//! [`MailSync`]'s keep-in-sync loop: a wake, or the poll interval going
//! by, while a long backfill runs gets a pass at once, between two of its
//! batches; backfill then carries on, and its count with it.
//!
//! The server offers no IDLE, so every pass comes from the test's wake or
//! the poll interval.

use std::time::Duration;

use postrinch_mailtest::Extension;
use postrinch_sync::{KeepOptions, Phase, Progress, SyncEvent, SyncReport};
use tokio::sync::Notify;

use super::idle_support::{KEEP, keeping, mail_sync};
use super::keep_support::{Events, kinds, until, until_done, until_true};
use super::sync_support::{Fixture, fixture_without, mail};

/// More than one batch of bodies (20 a batch).
const OLD: usize = 50;

/// A fake without IDLE, with [`OLD`] messages in the inbox.
async fn old_mail() -> Fixture {
    let f = fixture_without(&[Extension::Idle]).await;
    for n in 0..OLD {
        let id = format!("old{n}@x");
        f.user.append("INBOX", mail(Some(&id), "Old", "")).unwrap();
    }
    f
}

/// Whether the server has been asked for a batch of bodies.
fn backfilling(f: &Fixture) -> bool {
    f.server
        .commands()
        .iter()
        .any(|c| c.starts_with("UID FETCH") && c.contains("BODY.PEEK[]"))
}

/// What each sync found.
fn synced(log: &Events) -> Vec<SyncReport> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            SyncEvent::Synced { report, .. } => Some(report.clone()),
            _ => None,
        })
        .collect()
}

/// The bodies' count, as the sidebar shows it: `done` of `total`.
fn bodies(log: &Events) -> Vec<(usize, usize)> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            SyncEvent::Progress {
                progress:
                    Progress::Working {
                        phase: Phase::Bodies,
                        done,
                        total,
                    },
                ..
            } => Some((*done, *total)),
            _ => None,
        })
        .collect()
}

/// Wakes the loop, and returns once the wake has reached each account's
/// own loop. It is passed on twice (`MailSync::keep_in_sync`'s relay,
/// then the keeper's), each by a future polled in this test's own task
/// beside the test (`keeping`), on a runtime of one thread: so each time
/// this task is polled, both are, and a wake passed on wakes the task
/// again. Three polls see it through, whatever order they come in.
async fn wake_every_account(wake: &Notify) {
    wake.notify_one();
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn a_wake_during_a_long_backfill_gets_a_pass_before_it_finishes() {
    let f = old_mail().await;
    // The backfill's first batch waits on the server while the test
    // wakes the loop, as a send or Sync now would.
    let first_batch = f.server.hold_next("UID FETCH", "BODY.PEEK[]");
    let log = keeping(&mail_sync(&f), KEEP, async |events, wake| {
        until(events, |k| k.contains(&"synced")).await;
        until_true("the first batch is asked for", || backfilling(&f)).await;
        f.user
            .append("INBOX", mail(Some("new@x"), "New", ""))
            .unwrap();
        wake_every_account(&wake).await;
        first_batch.let_go();
        until_done(events, 1).await;
        // The wake brought one pass, not two: the poll is an hour away.
        tokio::time::sleep(Duration::from_millis(300)).await;
    })
    .await;
    // The pass the wake asked for came between two batches, before
    // backfill finished: no Done for the pass that gave way.
    assert_eq!(
        kinds(&log),
        ["started", "synced", "started", "synced", "done"]
    );
    let found = synced(&log);
    assert_eq!(found[0].headers, OLD);
    assert_eq!(found[1].headers, 1, "the new mail, stored by that pass");
    // Then backfill carried on, and finished: every body is here.
    assert!(f.message("new@x").unwrap().downloaded);
    assert!(f.store.undownloaded(&f.me).unwrap().is_empty());
    // The count went on from where it was, the new message added.
    let counts = bodies(&log);
    assert_eq!(counts[..2], [(0, OLD), (20, OLD)], "{counts:?}");
    assert_eq!(counts[2], (20, OLD + 1), "{counts:?}");
    assert_eq!(counts.last(), Some(&(OLD + 1, OLD + 1)), "{counts:?}");
    assert!(counts.windows(2).all(|w| w[0].0 <= w[1].0), "{counts:?}");
}

#[tokio::test]
async fn the_poll_interval_going_by_during_a_backfill_gets_a_pass() {
    let f = old_mail().await;
    let first_batch = f.server.hold_next("UID FETCH", "BODY.PEEK[]");
    let options = KeepOptions {
        interval: Duration::from_millis(300),
        ..KEEP
    };
    let log = keeping(&mail_sync(&f), options, async |events, _| {
        until_true("the first batch is asked for", || backfilling(&f)).await;
        f.user
            .append("INBOX", mail(Some("new@x"), "New", ""))
            .unwrap();
        // The interval goes by with the batch held: no wake.
        tokio::time::sleep(options.interval).await;
        first_batch.let_go();
        until_done(events, 1).await;
    })
    .await;
    let kinds = kinds(&log);
    assert_eq!(kinds[..4], ["started", "synced", "started", "synced"]);
    assert_eq!(synced(&log)[1].headers, 1, "the new mail, polled for");
    assert!(f.store.undownloaded(&f.me).unwrap().is_empty());
}

#[tokio::test]
async fn a_sync_as_long_as_the_poll_interval_leaves_backfill_a_whole_one() {
    let f = old_mail().await;
    // The sync's headers come only once the interval has gone by, as on
    // a slow server: the interval counts from when backfill begins, so
    // backfill does not give way after its first batch.
    let headers = f.server.hold_next("UID FETCH", "BODY.PEEK[HEADER]");
    let options = KeepOptions {
        interval: Duration::from_secs(2),
        ..KEEP
    };
    let log = keeping(&mail_sync(&f), options, async |events, _| {
        until_true("the headers are asked for", || {
            f.server
                .commands()
                .iter()
                .any(|c| c.starts_with("UID FETCH") && c.contains("BODY.PEEK[HEADER]"))
        })
        .await;
        tokio::time::sleep(options.interval).await;
        headers.let_go();
        until_done(events, 1).await;
    })
    .await;
    assert_eq!(kinds(&log)[..3], ["started", "synced", "done"]);
    let counts = bodies(&log);
    assert_eq!(
        counts,
        [(0, OLD), (20, OLD), (40, OLD), (OLD, OLD)],
        "every batch in one pass"
    );
}
