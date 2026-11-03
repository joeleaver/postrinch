//! Keeping accounts in sync: passes, retries, and accounts left alone.

use std::sync::{Arc, Mutex};
use std::time::Duration;

mod common;

use common::*;
use postrinch_model::{Account, AccountState, EmailAddress, Provider, SignInReason};
use postrinch_sync::google::{KeepOptions, SyncEvent};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const FAST: KeepOptions = KeepOptions {
    interval: Duration::from_secs(3600),
    retry: Duration::from_millis(50),
};

/// Run the loop for `time`, collecting what it reports.
async fn run_for(f: &Fixture, time: Duration) -> Vec<SyncEvent> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&events);
    let _ = tokio::time::timeout(
        time,
        f.sync
            .keep_in_sync(FAST, Arc::new(move |e| log.lock().unwrap().push(e))),
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
