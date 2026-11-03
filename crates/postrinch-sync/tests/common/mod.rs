//! Helpers shared by the sync tests: a mock of the Gmail API and a store.
#![allow(dead_code)]

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use postrinch_model::{Account, EmailAddress, Provider};
use postrinch_store::Store;
use postrinch_sync::google::{
    ClientConfig, Endpoints, GmailApi, GmailSync, GoogleAuth, MemorySecrets, SecretStore,
};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub struct Fixture {
    pub server: MockServer,
    pub store: Store,
    pub sync: GmailSync,
    pub me: EmailAddress,
    pub _dir: tempfile::TempDir,
}

pub async fn fixture() -> Fixture {
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
    // Headers only, unless a test asks for the newest whole.
    let sync = GmailSync::new(api, store.clone()).with_whole_first(0);
    Fixture {
        server,
        store,
        sync,
        me,
        _dir: dir,
    }
}

pub fn raw(subject: &str, body: &str) -> String {
    let message = format!(
        "From: Ada <ada@example.org>\r\nTo: me@example.org\r\nSubject: {subject}\r\n\r\n{body}\r\n"
    );
    URL_SAFE_NO_PAD.encode(message)
}

pub async fn get(server: &MockServer, p: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(format!("/gmail/{p}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

pub async fn mock_labels(server: &MockServer) {
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

pub async fn mock_raw(server: &MockServer, id: &str, labels: &[&str], subject: &str) {
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

pub async fn mock_meta(server: &MockServer, id: &str, labels: &[&str], subject: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/gmail/messages/{id}")))
        .and(query_param("format", "metadata"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id, "threadId": format!("t-{id}"), "labelIds": labels, "snippet": format!("snippet {id}"),
            "internalDate": "1700000000000", "sizeEstimate": 2048,
            "payload": {"headers": [
                {"name": "From", "value": "Ada <ada@example.org>"},
                {"name": "To", "value": "me@example.org"},
                {"name": "Subject", "value": subject},
            ]},
        })))
        .mount(server)
        .await;
}

pub async fn mock_history(server: &MockServer, start: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/gmail/history"))
        .and(query_param("startHistoryId", start))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

pub fn quiet() -> postrinch_sync::google::ProgressFn {
    Arc::new(|_| {})
}
