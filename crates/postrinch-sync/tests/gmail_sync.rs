//! Gmail sync against a mock of the Gmail API, into a real store.

use std::sync::{Arc, Mutex};

mod common;

use common::*;
use postrinch_model::{AccountState, ProviderMessageId, SignInReason};
use postrinch_sync::SyncError;
use postrinch_sync::google::{Phase, Progress, SyncReport};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

/// A full first sync over two pages, then the history since it began.
async fn first_sync(f: &Fixture) -> SyncReport {
    get(
        &f.server,
        "profile",
        json!({"emailAddress": "me@example.org", "historyId": "100"}),
    )
    .await;
    mock_labels(&f.server).await;
    Mock::given(method("GET"))
        .and(path("/gmail/messages"))
        .and(query_param("pageToken", "p2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"messages": [{"id": "m2", "threadId": "t-m2"}]})),
        )
        .mount(&f.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"messages": [{"id": "m1", "threadId": "t-m1"}], "nextPageToken": "p2"}),
        ))
        .mount(&f.server)
        .await;
    let m1_labels = ["INBOX", "UNREAD", "CHAT"];
    mock_meta(&f.server, "m1", &m1_labels, "Engine notes").await;
    mock_meta(&f.server, "m2", &["INBOX"], "Lunch").await;
    mock_raw(&f.server, "m1", &m1_labels, "Engine notes").await;
    mock_raw(&f.server, "m2", &["INBOX"], "Lunch").await;
    mock_history(&f.server, "100", json!({"historyId": "105"})).await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let report = f
        .sync
        .sync(&f.me, Arc::new(move |p| log.lock().unwrap().push(p)))
        .await
        .unwrap();
    assert!(seen.lock().unwrap().contains(&Progress::Working {
        phase: Phase::Headers,
        done: 2,
        total: 2
    }));
    report
}

#[tokio::test]
async fn full_then_incremental() {
    let f = fixture().await;
    let report = first_sync(&f).await;
    assert_eq!(
        report,
        SyncReport {
            full: true,
            headers: 2,
            ..Default::default()
        }
    );

    // Headers first: listed and searchable by subject, no body yet.
    let m1_id = ProviderMessageId("m1".into());
    let m1 = f.store.message(&f.me, &m1_id).unwrap().unwrap();
    assert_eq!(m1.subject(), "Engine notes");
    assert_eq!(m1.headers.from[0].address.to_string(), "ada@example.org");
    assert_eq!(m1.labels, ["INBOX"]);
    assert!(m1.flags.unread);
    assert!(!m1.downloaded);
    assert!(f.store.raw_message(&f.me, &m1.id).unwrap().is_none());
    assert_eq!(f.store.labels(&f.me).unwrap().len(), 2);
    assert_eq!(f.store.search("lunch", 5).unwrap().len(), 1);
    assert_eq!(f.store.undownloaded(&f.me).unwrap().len(), 2);

    // Then the bodies.
    let report = f.sync.backfill(&f.me, quiet()).await.unwrap();
    assert_eq!(report.fetched, 2);
    let m1 = f.store.message(&f.me, &m1_id).unwrap().unwrap();
    assert!(m1.downloaded);
    assert!(f.store.raw_message(&f.me, &m1.id).unwrap().is_some());
    assert_eq!(f.store.search("body", 5).unwrap().len(), 2);
    assert!(f.store.undownloaded(&f.me).unwrap().is_empty());
    assert_eq!(
        f.sync.backfill(&f.me, quiet()).await.unwrap(),
        SyncReport::default()
    );

    // m1 read and labelled, m2 deleted, m3 arrived.
    mock_history(
        &f.server,
        "105",
        json!({"historyId": "110", "history": [
            {"labelsAdded": [{"message": {"id": "m1"}, "labelIds": ["Label_1"]}]},
            {"messagesDeleted": [{"message": {"id": "m2"}}]},
            {"messagesAdded": [{"message": {"id": "m3"}}]}
        ]}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/gmail/messages/m1"))
        .and(query_param("format", "minimal"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "m1", "labelIds": ["INBOX", "Label_1", "STARRED"]})),
        )
        .mount(&f.server)
        .await;
    mock_raw(&f.server, "m3", &["INBOX"], "New").await;

    let report = f.sync.sync(&f.me, quiet()).await.unwrap();
    assert_eq!(
        report,
        SyncReport {
            full: false,
            fetched: 1,
            relabelled: 1,
            removed: 1,
            ..Default::default()
        }
    );
    let m1 = f
        .store
        .message(&f.me, &ProviderMessageId("m1".into()))
        .unwrap()
        .unwrap();
    assert_eq!(m1.labels, ["INBOX", "Label_1"]);
    assert!(!m1.flags.unread && m1.flags.starred);
    assert!(
        !f.store
            .has_message(&f.me, &ProviderMessageId("m2".into()))
            .unwrap()
    );
    assert!(
        f.store
            .has_message(&f.me, &ProviderMessageId("m3".into()))
            .unwrap()
    );

    // Nothing changed: only the cursor moves.
    mock_history(&f.server, "110", json!({"historyId": "111"})).await;
    assert_eq!(
        f.sync.sync(&f.me, quiet()).await.unwrap(),
        SyncReport::default()
    );
}

#[tokio::test]
async fn expired_history_falls_back_to_a_full_sync_that_refreshes_labels() {
    let f = fixture().await;
    first_sync(&f).await;
    f.server.reset().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "at", "expires_in": 3599})),
        )
        .mount(&f.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/gmail/history"))
        .and(query_param("startHistoryId", "105"))
        .respond_with(ResponseTemplate::new(404).set_body_json(
            json!({"error": {"code": 404, "message": "Requested entity was not found."}}),
        ))
        .mount(&f.server)
        .await;
    get(&f.server, "profile", json!({"historyId": "500"})).await;
    mock_labels(&f.server).await;
    get(
        &f.server,
        "messages",
        json!({"messages": [{"id": "m1", "threadId": "t-m1"}]}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/gmail/messages/m1"))
        .and(query_param("format", "minimal"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "m1", "labelIds": ["Label_1"]})),
        )
        .mount(&f.server)
        .await;
    mock_history(&f.server, "500", json!({"historyId": "501"})).await;

    let report = f.sync.sync(&f.me, quiet()).await.unwrap();
    assert_eq!(
        report,
        SyncReport {
            full: true,
            relabelled: 1,
            removed: 1,
            ..Default::default()
        }
    );
    let m1 = f
        .store
        .message(&f.me, &ProviderMessageId("m1".into()))
        .unwrap()
        .unwrap();
    assert_eq!(m1.labels, ["Label_1"]);
    assert_eq!(f.store.message_ids(&f.me).unwrap().len(), 1);
}

#[tokio::test]
async fn rate_limits_are_retried() {
    let f = fixture().await;
    Mock::given(method("GET"))
        .and(path("/gmail/profile"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&f.server)
        .await;
    get(&f.server, "profile", json!({"historyId": "1"})).await;
    mock_labels(&f.server).await;
    get(&f.server, "messages", json!({})).await;
    mock_history(&f.server, "1", json!({"historyId": "2"})).await;

    let report = f.sync.sync(&f.me, quiet()).await.unwrap();
    assert_eq!(
        report,
        SyncReport {
            full: true,
            ..Default::default()
        }
    );
}

#[tokio::test]
async fn a_revoked_token_marks_the_account() {
    let f = fixture().await;
    f.server.reset().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})))
        .mount(&f.server)
        .await;

    let err = f.sync.sync(&f.me, quiet()).await.unwrap_err();
    assert!(
        matches!(
            err,
            SyncError::NeedsSignIn(SignInReason::CredentialRejected { .. })
        ),
        "{err:?}"
    );
    let account = f.store.account(&f.me).unwrap().unwrap();
    assert!(matches!(
        account.state,
        AccountState::NeedsSignIn(SignInReason::CredentialRejected { .. })
    ));
}

#[tokio::test]
async fn the_newest_messages_come_whole() {
    let mut f = fixture().await;
    f.sync = f.sync.clone().with_whole_first(1);
    let report = first_sync(&f).await;
    // m1 is listed first, so newest: whole. m2 by its headers.
    assert_eq!((report.fetched, report.headers), (1, 1));
    let m1 = f
        .store
        .message(&f.me, &ProviderMessageId("m1".into()))
        .unwrap()
        .unwrap();
    assert!(m1.downloaded);
    let m2 = f
        .store
        .message(&f.me, &ProviderMessageId("m2".into()))
        .unwrap()
        .unwrap();
    assert!(!m2.downloaded);
    assert_eq!(f.store.undownloaded(&f.me).unwrap().len(), 1);
}

#[tokio::test]
async fn an_opened_message_downloads_at_once() {
    let f = fixture().await;
    first_sync(&f).await;
    let m2 = ProviderMessageId("m2".into());
    assert_eq!(
        f.sync.download_now(&f.me, vec![m2.clone()]).await.unwrap(),
        1
    );
    assert!(f.store.message(&f.me, &m2).unwrap().unwrap().downloaded);
    // Only that one: m1 still waits for backfill.
    assert_eq!(
        f.store.undownloaded(&f.me).unwrap(),
        [ProviderMessageId("m1".into())]
    );
}
