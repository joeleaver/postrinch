//! Gmail's keep-in-sync loop and backfill (`google::sync::keep`,
//! `giving_way`): every account syncs before any backfills, a wake
//! during backfill gets a pass before it finishes, and a wake backfill
//! took is answered whatever fails after it, and a failed download
//! sees those under way through.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod common;

use common::*;
use postrinch_model::{Account, EmailAddress, Provider, ProviderMessageId};
use postrinch_sync::{IdleOptions, KeepOptions, Phase, Progress, SecretStore, SyncEvent};
use serde_json::json;
use tokio::sync::Notify;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

/// An hour between passes, and between a failed one and the next: any
/// pass sooner is one a wake or a backfill giving way asked for.
const SLOW: KeepOptions = KeepOptions {
    interval: Duration::from_secs(3600),
    retry: Duration::from_secs(3600),
    idle: IdleOptions::DEFAULT,
};

/// More messages than Gmail backfill fetches at once (10).
const MANY: usize = 12;

/// Gmail's quota, many times over, so downloading [`MANY`] messages
/// twice over waits on no pacing.
const QUOTA: f64 = 100.0;

type Events = Arc<Mutex<Vec<SyncEvent>>>;

/// The ids of [`MANY`] messages, in the order backfill takes them: all
/// came at once, so by id.
fn ids() -> Vec<String> {
    let mut ids: Vec<String> = (0..MANY).map(|n| format!("m{n}")).collect();
    ids.sort();
    ids
}

/// An account of [`MANY`] messages, stored by their headers first; the
/// bodies of all but `failing` come `delay` after they are asked for.
async fn mock_many_messages(f: &Fixture, failing: Option<&str>, delay: Duration) {
    get(
        &f.server,
        "profile",
        json!({"emailAddress": "me@example.org", "historyId": "100"}),
    )
    .await;
    mock_labels(&f.server).await;
    let listed: Vec<_> = ids()
        .iter()
        .map(|id| json!({"id": id, "threadId": format!("t-{id}")}))
        .collect();
    get(&f.server, "messages", json!({ "messages": listed })).await;
    for id in &ids() {
        mock_meta(&f.server, id, &["INBOX"], "Old").await;
        if failing != Some(id) {
            mock_raw_after(&f.server, id, &["INBOX"], "Old", delay).await;
        }
    }
    mock_history(&f.server, "100", json!({"historyId": "100"})).await;
}

/// `id`'s body fails, `delay` after it is asked for.
async fn mock_failing_body(f: &Fixture, id: &str, delay: Duration) {
    Mock::given(method("GET"))
        .and(path(format!("/gmail/messages/{id}")))
        .and(query_param("format", "raw"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string("no")
                .set_delay(delay),
        )
        .mount(&f.server)
        .await;
}

/// Runs the loop until `done` holds of what it reported (up to [`HUNG`]).
async fn keeping(
    f: &Fixture,
    report: impl Fn(&SyncEvent) + Send + Sync + 'static,
    wake: Arc<Notify>,
    done: impl Fn(&[SyncEvent]) -> bool,
) -> Vec<SyncEvent> {
    let events: Events = Arc::default();
    let log = Arc::clone(&events);
    let on_event = move |e: SyncEvent| {
        report(&e);
        log.lock().unwrap().push(e);
    };
    let until = async {
        while !done(&events.lock().unwrap()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let ran = tokio::time::timeout(HUNG, async {
        tokio::select! {
            () = f.sync.keep_in_sync(SLOW, Arc::new(on_event), wake) => {}
            () = until => {}
        }
    })
    .await;
    let events = events.lock().unwrap().clone();
    // Each account's last count too: a backfill still moving was slow, one
    // stuck where it began hung.
    assert!(
        ran.is_ok(),
        "gave up waiting: {:?}, bodies by account {:?}",
        kinds(&events),
        bodies_by_account(&events)
    );
    events
}

fn count(events: &[SyncEvent], kind: &str) -> usize {
    kinds(events).iter().filter(|k| **k == kind).count()
}

/// What happened, by kind, progress left out.
fn kinds(events: &[SyncEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|e| match e {
            SyncEvent::Started { .. } => Some("started"),
            SyncEvent::Synced { .. } => Some("synced"),
            SyncEvent::Done { .. } => Some("done"),
            SyncEvent::Failed { .. } => Some("failed"),
            _ => None,
        })
        .collect()
}

/// The bodies' count, as the sidebar shows it: `done` of `total`.
fn bodies(events: &[SyncEvent]) -> Vec<(usize, usize)> {
    events
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

/// Each account's last bodies' count, `done` of `total`.
fn bodies_by_account(events: &[SyncEvent]) -> Vec<(String, (usize, usize))> {
    let mut last: Vec<(String, (usize, usize))> = Vec::new();
    for e in events {
        if let SyncEvent::Progress {
            account,
            progress:
                Progress::Working {
                    phase: Phase::Bodies,
                    done,
                    total,
                },
        } = e
        {
            let account = account.to_string();
            match last.iter_mut().find(|(a, _)| *a == account) {
                Some((_, count)) => *count = (*done, *total),
                None => last.push((account, (*done, *total))),
            }
        }
    }
    last
}

/// Wakes `wake` once, when the first sync is reported: as Sync now
/// would, with backfill about to begin.
fn wake_at_first_sync(wake: &Arc<Notify>) -> impl Fn(&SyncEvent) + Send + Sync + 'static {
    let (waker, woken) = (Arc::clone(wake), AtomicBool::new(false));
    move |e| {
        if matches!(e, SyncEvent::Synced { .. }) && !woken.swap(true, Ordering::Relaxed) {
            waker.notify_one();
        }
    }
}

#[tokio::test]
async fn a_wake_during_backfill_gets_a_pass_before_it_finishes() {
    let f = fixture_with_quota(QUOTA).await;
    mock_many_messages(&f, None, Duration::ZERO).await;
    let wake = Arc::new(Notify::new());
    let report = wake_at_first_sync(&wake);
    let events = keeping(&f, report, wake, |e| count(e, "done") == 1).await;
    // The pass came before backfill finished, and then backfill finished:
    // one wake, one more pass.
    assert_eq!(
        kinds(&events),
        ["started", "synced", "started", "synced", "done"]
    );
    let downloaded = events.iter().find_map(|e| match e {
        SyncEvent::Done { downloaded, .. } => Some(*downloaded),
        _ => None,
    });
    assert!(
        downloaded.is_some_and(|d| d > 0 && d < MANY),
        "the rest, after the pass: {downloaded:?}"
    );
    assert!(f.store.undownloaded(&f.me).unwrap().is_empty());
    // The count went on across the pass, to every message.
    assert_eq!(bodies(&events).last(), Some(&(MANY, MANY)));
}

#[tokio::test]
async fn every_account_syncs_before_any_backfills() {
    let f = fixture_with_quota(QUOTA).await;
    mock_many_messages(&f, None, Duration::ZERO).await;
    let other = EmailAddress::parse("other@example.org").unwrap();
    f.store
        .put_account(&Account::new(other.clone(), Provider::Google))
        .unwrap();
    f.secrets.set(&other, "rt").unwrap();
    let events = keeping(&f, |_| {}, Arc::default(), |e| count(e, "done") == 2).await;
    // A wake for the second account is never held up by the first's
    // backfill: both sync, then both backfill.
    assert_eq!(
        kinds(&events),
        ["started", "synced", "started", "synced", "done", "done"]
    );
    let first_body = events
        .iter()
        .position(|e| {
            matches!(
                e,
                SyncEvent::Progress {
                    progress: Progress::Working {
                        phase: Phase::Bodies,
                        ..
                    },
                    ..
                }
            )
        })
        .unwrap();
    let last_sync = events
        .iter()
        .rposition(|e| matches!(e, SyncEvent::Synced { .. }))
        .unwrap();
    assert!(last_sync < first_body, "{:?}", kinds(&events));
    assert!(f.store.undownloaded(&f.me).unwrap().is_empty());
    assert!(f.store.undownloaded(&other).unwrap().is_empty());
}

#[tokio::test]
async fn a_wake_backfill_took_gets_its_pass_though_a_download_then_fails() {
    let f = fixture_with_quota(QUOTA).await;
    // The first message backfill starts fails, after the others are in:
    // so backfill has taken the wake (after the first download) when the
    // failure ends it.
    let failing = ids()[0].clone();
    mock_many_messages(&f, Some(&failing), Duration::ZERO).await;
    mock_failing_body(&f, &failing, Duration::from_millis(300)).await;
    let wake = Arc::new(Notify::new());
    let report = wake_at_first_sync(&wake);
    // Each pass's backfill fails on that message; the retry is an hour
    // away, so only the wake explains a second pass.
    let events = keeping(&f, report, wake, |e| count(e, "failed") == 2).await;
    assert_eq!(
        kinds(&events),
        ["started", "synced", "failed", "started", "synced", "failed"]
    );
    // The count carried on from where the failed backfill got to, never
    // back to nothing, and each backfill counted every download under way
    // when it failed, however late it came.
    let counts = bodies(&events);
    assert!(counts.windows(2).all(|w| w[0].0 <= w[1].0), "{counts:?}");
    assert_eq!(counts.last(), Some(&(MANY - 1, MANY)), "{counts:?}");
    assert_eq!(
        f.store.undownloaded(&f.me).unwrap(),
        [ProviderMessageId(failing)]
    );
}

#[tokio::test]
async fn a_failed_download_sees_those_under_way_through() {
    let f = fixture_with_quota(QUOTA).await;
    // The first message backfill starts fails at once; the others started
    // with it come after.
    let failing = ids()[0].clone();
    mock_many_messages(&f, Some(&failing), Duration::from_millis(200)).await;
    mock_failing_body(&f, &failing, Duration::ZERO).await;
    f.sync.sync(&f.me, quiet()).await.unwrap();
    let events: Events = Arc::default();
    let (log, account) = (Arc::clone(&events), f.me.clone());
    let progress = Arc::new(move |progress| {
        let account = account.clone();
        log.lock()
            .unwrap()
            .push(SyncEvent::Progress { account, progress });
    });
    assert!(f.sync.backfill(&f.me, progress).await.is_err());
    // Every download started besides the failing one, under way when it
    // failed, was seen through: stored, and counted.
    let started = f
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| {
            r.url.query() == Some("format=raw")
                && r.url.path() != format!("/gmail/messages/{failing}")
        })
        .count();
    let stored = MANY - f.store.undownloaded(&f.me).unwrap().len();
    let counts = bodies(&events.lock().unwrap());
    assert!(started > 0);
    assert_eq!(stored, started);
    assert_eq!(counts.last(), Some(&(started, MANY)), "{counts:?}");
    assert!(
        f.store
            .undownloaded(&f.me)
            .unwrap()
            .contains(&ProviderMessageId(failing))
    );
}
