//! Keeping accounts in sync: passes, retries, accounts left alone, and
//! backfill giving way to a pass.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod common;

use common::*;
use postrinch_model::{Account, AccountState, EmailAddress, Provider, SignInReason};
use postrinch_sync::{IdleOptions, KeepOptions, Progress, SyncEvent};
use serde_json::json;
use tokio::sync::Notify;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const FAST: KeepOptions = KeepOptions {
    interval: Duration::from_secs(3600),
    retry: Duration::from_millis(50),
    idle: IdleOptions::DEFAULT,
};

/// Run the loop for `time`, collecting what it reports.
async fn run_for(f: &Fixture, time: Duration) -> Vec<SyncEvent> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&events);
    let wake = Arc::new(Notify::new());
    let _ = tokio::time::timeout(
        time,
        f.sync
            .keep_in_sync(FAST, Arc::new(move |e| log.lock().unwrap().push(e)), wake),
    )
    .await;
    Arc::try_unwrap(events).unwrap().into_inner().unwrap()
}

async fn mock_one_message(f: &Fixture) {
    get(
        &f.server,
        "profile",
        json!({"emailAddress": "me@example.org", "historyId": "100"}),
    )
    .await;
    mock_labels(&f.server).await;
    get(
        &f.server,
        "messages",
        json!({"messages": [{"id": "m1", "threadId": "t-m1"}]}),
    )
    .await;
    mock_meta(&f.server, "m1", &["INBOX"], "Engine notes").await;
    mock_raw(&f.server, "m1", &["INBOX"], "Engine notes").await;
    mock_history(&f.server, "100", json!({"historyId": "100"})).await;
}

#[tokio::test]
async fn a_pass_syncs_then_downloads_and_waits() {
    let f = fixture().await;
    mock_one_message(&f).await;
    let events = run_for(&f, Duration::from_secs(2)).await;

    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            SyncEvent::Started { .. } => Some("started"),
            SyncEvent::Synced { report, .. } => {
                assert_eq!(report.headers, 1);
                Some("synced")
            }
            SyncEvent::Done { downloaded, .. } => {
                assert_eq!(*downloaded, 1);
                Some("done")
            }
            SyncEvent::Progress { .. } => None,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    // One pass: the next waits an hour.
    assert_eq!(kinds, ["started", "synced", "done"]);
}

#[tokio::test]
async fn a_failed_pass_is_retried() {
    let f = fixture().await;
    // Gmail refuses the profile request outright, so every pass fails.
    Mock::given(method("GET"))
        .and(path("/gmail/profile"))
        .respond_with(ResponseTemplate::new(400).set_body_string("no"))
        .mount(&f.server)
        .await;
    let events = run_for(&f, Duration::from_millis(500)).await;
    let failures = events
        .iter()
        .filter(|e| matches!(e, SyncEvent::Failed { retry_in, .. } if *retry_in == FAST.retry))
        .count();
    assert!(failures >= 2, "expected retries, got {events:?}");
}

#[tokio::test]
async fn accounts_needing_sign_in_are_left_alone() {
    let f = fixture().await;
    mock_one_message(&f).await;
    let other = EmailAddress::parse("other@example.org").unwrap();
    let mut account = Account::new(other.clone(), Provider::Google);
    account.needs_sign_in(SignInReason::NoCredential);
    f.store.put_account(&account).unwrap();

    let events = run_for(&f, Duration::from_secs(2)).await;
    assert!(events.iter().all(|e| !matches!(e,
        SyncEvent::Started { account } if *account == other)));
    assert!(matches!(
        f.store.account(&other).unwrap().unwrap().state,
        AccountState::NeedsSignIn(_)
    ));
}

#[tokio::test]
async fn waking_starts_the_next_pass_at_once() {
    let f = fixture().await;
    mock_one_message(&f).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&events);
    let wake = Arc::new(Notify::new());
    let waker = Arc::clone(&wake);
    let ring = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        waker.notify_one();
    });
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        f.sync
            .keep_in_sync(FAST, Arc::new(move |e| log.lock().unwrap().push(e)), wake),
    )
    .await;
    ring.await.unwrap();
    let started = events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, SyncEvent::Started { .. }))
        .count();
    // FAST waits an hour between passes; only the wake explains a second.
    assert_eq!(started, 2);
}

/// More messages than Gmail backfill fetches at once (10), and few
/// enough that the quota's pacing keeps the test short.
const MANY: usize = 12;

/// An account of [`MANY`] messages, stored by their headers first.
async fn mock_many_messages(f: &Fixture) {
    get(
        &f.server,
        "profile",
        json!({"emailAddress": "me@example.org", "historyId": "100"}),
    )
    .await;
    mock_labels(&f.server).await;
    let ids: Vec<String> = (0..MANY).map(|n| format!("m{n}")).collect();
    let listed: Vec<_> = ids
        .iter()
        .map(|id| json!({"id": id, "threadId": format!("t-{id}")}))
        .collect();
    get(&f.server, "messages", json!({ "messages": listed })).await;
    for id in &ids {
        mock_meta(&f.server, id, &["INBOX"], "Old").await;
        mock_raw(&f.server, id, &["INBOX"], "Old").await;
    }
    mock_history(&f.server, "100", json!({"historyId": "100"})).await;
}

#[tokio::test]
async fn a_wake_during_backfill_gets_a_pass_before_it_finishes() {
    let f = fixture().await;
    mock_many_messages(&f).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&events);
    let wake = Arc::new(Notify::new());
    let waker = Arc::clone(&wake);
    let woken = AtomicBool::new(false);
    // Woken as the first backfill begins, as Sync now would be.
    let report = move |e: SyncEvent| {
        if matches!(e, SyncEvent::Synced { .. }) && !woken.swap(true, Ordering::Relaxed) {
            waker.notify_one();
        }
        log.lock().unwrap().push(e);
    };
    let done = async {
        while !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, SyncEvent::Done { .. }))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // No pass more: the poll is an hour away.
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    let ran = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::select! {
            () = f.sync.keep_in_sync(FAST, Arc::new(report), wake) => {}
            () = done => {}
        }
    })
    .await;
    assert!(ran.is_ok(), "gave up waiting for the pass to be done");
    let events = events.lock().unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            SyncEvent::Started { .. } => Some("started"),
            SyncEvent::Synced { .. } => Some("synced"),
            SyncEvent::Done { .. } => Some("done"),
            _ => None,
        })
        .collect();
    // The pass came before backfill finished, and then backfill finished:
    // one wake, one more pass.
    assert_eq!(kinds, ["started", "synced", "started", "synced", "done"]);
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
    let last = events.iter().rev().find_map(|e| match e {
        SyncEvent::Progress {
            progress: Progress::Working { done, total, .. },
            ..
        } => Some((*done, *total)),
        _ => None,
    });
    assert_eq!(last, Some((MANY, MANY)));
}
