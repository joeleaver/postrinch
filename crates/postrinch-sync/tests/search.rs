//! Asking a mock of the Gmail API to search.

mod common;

use common::*;
use postrinch_model::ProviderMessageId;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn gmail_search_hits_are_stored_once_by_their_headers() {
    let f = fixture().await;
    mock_labels(&f.server).await;
    mock_meta(&f.server, "m1", &["INBOX"], "Cover needed").await;
    mock_meta(&f.server, "m2", &["INBOX", "UNREAD"], "Rota for March").await;
    Mock::given(method("GET"))
        .and(path("/gmail/messages"))
        .and(query_param("q", "from:rota shift"))
        .and(query_param("maxResults", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "messages": [{"id": "m2", "threadId": "t-m2"}, {"id": "m1", "threadId": "t-m1"}]
        })))
        .expect(2)
        .mount(&f.server)
        .await;

    let ids = |v: &[&str]| -> Vec<ProviderMessageId> {
        v.iter().map(|s| ProviderMessageId((*s).into())).collect()
    };
    for _ in 0..2 {
        let found = f
            .sync
            .search_remote(&f.me, "from:rota shift", 50)
            .await
            .unwrap();
        assert_eq!(found, ids(&["m2", "m1"]), "Gmail's order, newest first");
    }
    let fetched = f
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path().starts_with("/gmail/messages/m"))
        .count();
    assert_eq!(fetched, 2, "each new hit's headers are fetched once");
    let m2 = f.store.message(&f.me, &ids(&["m2"])[0]).unwrap().unwrap();
    assert_eq!(m2.subject(), "Rota for March");
    assert!(!m2.downloaded);
    assert!(m2.flags.unread);
}
