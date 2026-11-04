//! Delivering mail written here to a mock of the Gmail API.

mod common;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::*;
use postrinch_model::{Draft, EmailAddress, Mailbox, ProviderMessageId, ProviderThreadId};
use postrinch_store::OutgoingKind;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

fn reply(f: &Fixture) -> Draft {
    let mut draft = Draft::new(f.me.clone());
    draft.to = vec![Mailbox::new(
        Some("Ada".into()),
        EmailAddress::parse("ada@example.org").unwrap(),
    )];
    draft.subject = "Re: Thursday".into();
    draft.body = "I can take it.".into();
    draft.answering.thread = Some(ProviderThreadId("t1".into()));
    draft
}

/// The raw message a request to Gmail carried, decoded.
fn sent_raw(body: &Value) -> String {
    let raw = body["raw"]
        .as_str()
        .or(body["message"]["raw"].as_str())
        .unwrap();
    String::from_utf8(URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap()
}

async fn requests_to(f: &Fixture, verb: &str, p: &str) -> Vec<Value> {
    f.server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == p)
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

#[tokio::test]
async fn a_message_goes_once_into_its_thread() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/gmail/messages/send"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "s1", "threadId": "t1"})),
        )
        .mount(&f.server)
        .await;
    f.store
        .queue_outgoing(&reply(&f), OutgoingKind::Send)
        .unwrap();
    f.store
        .queue_outgoing(&reply(&f), OutgoingKind::Proposed)
        .unwrap();

    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    let sent = requests_to(&f, "POST", "/gmail/messages/send").await;
    assert_eq!(sent.len(), 1, "the proposal is not sent");
    assert_eq!(sent[0]["threadId"], "t1");
    let raw = sent_raw(&sent[0]);
    assert!(raw.contains("Subject: Re: Thursday"), "{raw}");
    assert!(raw.contains("I can take it."));
    assert!(f.store.outgoing_for(&f.me).unwrap().is_empty());
    assert_eq!(
        f.store.outgoing().unwrap().len(),
        1,
        "the proposal waits for the person"
    );
}

#[tokio::test]
async fn a_send_that_may_have_gone_is_not_sent_again() {
    let f = fixture().await;
    // Gmail takes it, but the answer never arrives.
    Mock::given(method("POST"))
        .and(path("/gmail/messages/send"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(5)))
        .mount(&f.server)
        .await;
    let id = f
        .store
        .queue_outgoing(&reply(&f), OutgoingKind::Send)
        .unwrap();
    let message_id = {
        let o = &f.store.outgoing_for(&f.me).unwrap()[0];
        format!("postrinch.{}.{}@example.org", o.created.as_millis(), o.seq)
    };
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        f.sync.send_outgoing(&f.me),
    )
    .await;
    assert!(first.is_err(), "the answer was lost");
    f.store
        .outgoing_failed(id, "connection lost", false)
        .unwrap();

    // Gmail has it: the next pass finds it rather than sending it again.
    Mock::given(method("GET"))
        .and(path("/gmail/messages"))
        .and(query_param("q", format!("rfc822msgid:{message_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"messages": [{"id": "s1"}]})))
        .mount(&f.server)
        .await;
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(
        requests_to(&f, "POST", "/gmail/messages/send").await.len(),
        1
    );
    assert!(f.store.outgoing_for(&f.me).unwrap().is_empty());
}

#[tokio::test]
async fn a_message_gmail_refuses_waits_for_the_person() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/gmail/messages/send"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": {"message": "Invalid To header"}})),
        )
        .mount(&f.server)
        .await;
    f.store
        .queue_outgoing(&reply(&f), OutgoingKind::Send)
        .unwrap();
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = &f.store.outgoing_for(&f.me).unwrap()[0];
    assert!(waiting.refused);
    assert!(
        waiting
            .error
            .as_deref()
            .unwrap()
            .contains("Invalid To header")
    );
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    assert_eq!(
        requests_to(&f, "POST", "/gmail/messages/send").await.len(),
        1,
        "not tried again"
    );
}

#[tokio::test]
async fn drafts_are_kept_replaced_and_cleared_when_sent() {
    let f = fixture().await;
    get(
        &f.server,
        "drafts",
        json!({"drafts": [{"id": "d7", "message": {"id": "m7"}}]}),
    )
    .await;
    for (verb, p) in [("POST", "/gmail/drafts"), ("PUT", "/gmail/drafts/d7")] {
        Mock::given(method(verb))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id": "d7", "message": {"id": "m8"}})),
            )
            .mount(&f.server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/gmail/messages/send"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "s1"})))
        .mount(&f.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/gmail/drafts/d7"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&f.server)
        .await;

    let mut draft = reply(&f);
    f.store.queue_outgoing(&draft, OutgoingKind::Keep).unwrap();
    draft.replaces = Some(ProviderMessageId("m7".into()));
    f.store.queue_outgoing(&draft, OutgoingKind::Keep).unwrap();
    f.store.queue_outgoing(&draft, OutgoingKind::Send).unwrap();
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 3);

    assert_eq!(
        requests_to(&f, "POST", "/gmail/drafts").await.len(),
        1,
        "a new draft"
    );
    let put = requests_to(&f, "PUT", "/gmail/drafts/d7").await;
    assert_eq!(put.len(), 1, "the opened draft replaced");
    assert!(sent_raw(&put[0]).contains("I can take it."));
    assert_eq!(
        requests_to(&f, "DELETE", "/gmail/drafts/d7").await.len(),
        1,
        "sent, so the draft goes"
    );
}
