//! Gmail sync against a mock of the Gmail API, into a real store.

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use postrinch_model::{
    Account, AccountState, EmailAddress, Provider, ProviderMessageId, SignInReason,
};
use postrinch_store::Store;
use postrinch_sync::SyncError;
use postrinch_sync::google::{
    ClientConfig, Endpoints, GmailApi, GmailSync, GoogleAuth, MemorySecrets, Progress, SecretStore,
    SyncReport,
};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Fixture {
    server: MockServer,
    store: Store,
    sync: GmailSync,
    me: EmailAddress,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "at", "expires_in": 3599})),
        )
        .mount(&server)
        .await;
    let config = ClientConfig {
        client_id: "c".into(),
        client_secret: "s".into(),
        auth_uri: format!("{}/auth", server.uri()),
        token_uri: format!("{}/token", server.uri()),
    };
    let endpoints = Endpoints {
        auth: config.auth_uri.clone(),
        token: config.token_uri.clone(),
        userinfo: format!("{}/userinfo", server.uri()),
        revoke: format!("{}/revoke", server.uri()),
    };
    let me = EmailAddress::parse("me@example.org").unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    secrets.set(&me, "rt").unwrap();
    let auth = Arc::new(GoogleAuth::with_endpoints(config, endpoints, secrets));
    let api = Arc::new(GmailApi::with_base(auth, format!("{}/gmail", server.uri())));

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    let sync = GmailSync::new(api, store.clone());
    Fixture {
        server,
        store,
        sync,
        me,
        _dir: dir,
    }
}

fn raw(subject: &str, body: &str) -> String {
    let message = format!(
        "From: Ada <ada@example.org>\r\nTo: me@example.org\r\nSubject: {subject}\r\n\r\n{body}\r\n"
    );
    URL_SAFE_NO_PAD.encode(message)
}

async fn get(server: &MockServer, p: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(format!("/gmail/{p}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mock_labels(server: &MockServer) {
    get(
        server,
        "labels",
        json!({"labels": [
            {"id": "INBOX", "name": "INBOX", "type": "system"},
            {"id": "UNREAD", "name": "UNREAD", "type": "system"},
            {"id": "STARRED", "name": "STARRED", "type": "system"},
            {"id": "Label_1", "name": "Receipts", "type": "user"}
        ]}),
    )
    .await;
}

async fn mock_raw(server: &MockServer, id: &str, labels: &[&str], subject: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/gmail/messages/{id}")))
        .and(query_param("format", "raw"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id, "threadId": format!("t-{id}"), "labelIds": labels, "snippet": format!("snippet {id}"),
            "internalDate": "1700000000000", "raw": raw(subject, &format!("body of {id}")),
        })))
        .mount(server)
        .await;
}

async fn mock_history(server: &MockServer, start: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/gmail/history"))
        .and(query_param("startHistoryId", start))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn quiet() -> postrinch_sync::google::ProgressFn {
    Arc::new(|_| {})
}

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
    mock_raw(
        &f.server,
        "m1",
        &["INBOX", "UNREAD", "CHAT"],
        "Engine notes",
    )
    .await;
    mock_raw(&f.server, "m2", &["INBOX"], "Lunch").await;
    mock_history(&f.server, "100", json!({"historyId": "105"})).await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let report = f
        .sync
        .sync(&f.me, Arc::new(move |p| log.lock().unwrap().push(p)))
        .await
        .unwrap();
    assert!(
        seen.lock()
            .unwrap()
            .contains(&Progress::Applying { done: 2, total: 2 })
    );
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
            fetched: 2,
            relabelled: 0,
            removed: 0
        }
    );

    let m1 = f
        .store
        .message(&f.me, &ProviderMessageId("m1".into()))
        .unwrap()
        .unwrap();
    assert_eq!(m1.subject(), "Engine notes");
    assert_eq!(m1.labels, ["INBOX"]);
    assert!(m1.flags.unread);
    assert!(f.store.raw_message(&f.me, &m1.id).unwrap().is_some());
    assert_eq!(f.store.labels(&f.me).unwrap().len(), 2);
    assert_eq!(f.store.search("lunch", 5).unwrap().len(), 1);

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
            removed: 1
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
            fetched: 0,
            relabelled: 1,
            removed: 1
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
