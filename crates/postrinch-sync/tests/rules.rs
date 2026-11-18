//! Rules kept in Gmail: the settings message and the filters, against a
//! mock of the Gmail API.

mod common;

use common::*;
use postrinch_rules::{Action, Condition, Rule, Test};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mock_rules_basics(server: &MockServer, filters: serde_json::Value) {
    get(
        server,
        "labels",
        json!({"labels": [
            {"id": "INBOX", "name": "INBOX", "type": "system"},
            {"id": "Label_1", "name": "School", "type": "user"},
            {"id": "Label_S", "name": "[Postrinch]/Settings", "type": "user"}
        ]}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/gmail/messages"))
        .and(query_param("labelIds", "Label_S"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;
    get(server, "settings/filters", filters).await;
    get(
        server,
        "settings/forwardingAddresses",
        json!({"forwardingAddresses": [
            {"forwardingEmail": "books@example.com", "verificationStatus": "accepted"},
            {"forwardingEmail": "new@example.com", "verificationStatus": "pending"}
        ]}),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/gmail/messages"))
        .and(query_param("internalDateSource", "dateHeader"))
        .and(body_partial_json(json!({"labelIds": ["Label_S"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "settings-1"})))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn rules_become_filters_and_a_settings_message() {
    let f = fixture().await;
    mock_rules_basics(
        &f.server,
        json!({"filter": [{"id": "theirs", "criteria": {"from": "x@example.com"},
                            "action": {"removeLabelIds": ["INBOX"]}}]}),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/gmail/labels"))
        .and(body_partial_json(
            json!({"name": "[Postrinch]/r1", "labelListVisibility": "labelHide"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "Label_M"})))
        .expect(1)
        .mount(&f.server)
        .await;
    // Gmail adds one user label per filter: the label, then the marker.
    for (label, id) in [("Label_1", "ours-1"), ("Label_M", "ours-2")] {
        Mock::given(method("POST"))
            .and(path("/gmail/settings/filters"))
            .and(body_partial_json(json!({
                "criteria": {"query": "subject:(field trip)"},
                "action": {"addLabelIds": [label]}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id})))
            .expect(1)
            .mount(&f.server)
            .await;
    }

    let rule = Rule {
        conditions: vec![Condition::is(Test::SubjectWords("field trip".into()))],
        actions: vec![
            Action::AddLabel("Label_1".into()),
            Action::AskAgent {
                agent: "a".into(),
                prompt: "Summarise".into(),
            },
        ],
        ..Rule::new(1, "School")
    };
    f.store
        .update_rules(&f.me, |s, l| {
            s.rules.put(rule);
            l.dirty = true;
        })
        .unwrap();

    let report = f.sync.sync_rules(&f.me).await.unwrap();
    assert_eq!((report.made, report.deleted), (2, 0));
    let (settings, local) = f.store.rules(&f.me).unwrap();
    let ids: Vec<_> = settings.made.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["ours-1", "ours-2"]);
    assert!(settings.made[1].filter.marker);
    assert!(!local.dirty);
    assert_eq!(local.server, Some(true));
    assert_eq!(local.message.as_deref(), Some("settings-1"));
    assert_eq!(local.forwarding, ["books@example.com"]);
    assert_eq!(local.elsewhere.len(), 1);
    assert_eq!(
        local.elsewhere[0].line,
        "from:(x@example.com) \u{2192} skip the inbox"
    );
    let running = settings.running(true, local.elsewhere.len());
    assert!(matches!(
        running.part(1),
        Some(postrinch_rules::gmail::Part::AfterMarker(_))
    ));
}

#[tokio::test]
async fn without_the_settings_scope_rules_still_reach_the_account() {
    let f = fixture().await;
    get(
        &f.server,
        "labels",
        json!({"labels": [{"id": "Label_S", "name": "[Postrinch]/Settings", "type": "user"}]}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/gmail/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&f.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/settings/filters"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"code": 403, "message": "Request had insufficient authentication scopes.",
                      "errors": [{"reason": "insufficientPermissions"}]}
        })))
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/gmail/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "settings-2"})))
        .expect(1)
        .mount(&f.server)
        .await;
    let rule = Rule {
        conditions: vec![Condition::is(Test::FromAddress("a@example.com".into()))],
        actions: vec![Action::Star],
        ..Rule::new(1, "Stars")
    };
    f.store
        .update_rules(&f.me, |s, l| {
            s.rules.put(rule);
            l.dirty = true;
        })
        .unwrap();
    f.sync.sync_rules(&f.me).await.unwrap();
    let (settings, local) = f.store.rules(&f.me).unwrap();
    assert!(settings.made.is_empty());
    assert_eq!(local.server, Some(false));
    assert_eq!(local.message.as_deref(), Some("settings-2"));
    assert_eq!(
        settings.running(false, 0).part(1),
        Some(&postrinch_rules::gmail::Part::Whole)
    );
}

#[tokio::test]
async fn a_refused_forward_runs_here_with_a_reason() {
    let f = fixture().await;
    mock_rules_basics(&f.server, json!({})).await;
    Mock::given(method("POST"))
        .and(path("/gmail/settings/filters"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {"code": 400, "message": "Invalid forwarding address", "errors": [{"reason": "invalidArgument"}]}
        })))
        .mount(&f.server)
        .await;
    let rule = Rule {
        conditions: vec![Condition::is(Test::FromAddress(
            "billing@acme.example".into(),
        ))],
        actions: vec![Action::Forward("books@example.com".into())],
        ..Rule::new(4, "Invoices")
    };
    f.store
        .update_rules(&f.me, |s, l| {
            s.rules.put(rule);
            l.dirty = true;
        })
        .unwrap();
    f.sync.sync_rules(&f.me).await.unwrap();
    let (settings, _) = f.store.rules(&f.me).unwrap();
    assert!(settings.made.is_empty());
    assert!(settings.refused[&4].contains("books@example.com"));
}

#[tokio::test]
async fn a_deleted_rules_marker_goes() {
    let f = fixture().await;
    get(
        &f.server,
        "labels",
        json!({"labels": [
            {"id": "Label_S", "name": "[Postrinch]/Settings", "type": "user"},
            {"id": "Label_M", "name": "[Postrinch]/r7", "type": "user"}
        ]}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/gmail/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&f.server)
        .await;
    get(&f.server, "settings/filters", json!({})).await;
    get(&f.server, "settings/forwardingAddresses", json!({})).await;
    Mock::given(method("DELETE"))
        .and(path("/gmail/labels/Label_M"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&f.server)
        .await;
    f.sync.sync_rules(&f.me).await.unwrap();
}

#[tokio::test]
async fn an_account_without_rules_is_left_alone() {
    let f = fixture().await;
    get(&f.server, "labels", json!({"labels": []})).await;
    get(&f.server, "settings/filters", json!({})).await;
    get(&f.server, "settings/forwardingAddresses", json!({})).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&f.server)
        .await;
    f.sync.sync_rules(&f.me).await.unwrap();
}
