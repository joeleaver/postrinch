//! Sending changes made here to a mock of the Gmail API.

mod common;

use common::*;
use postrinch_model::{
    Flags, Label, LabelKind, Message, MessageChange, MessageHeaders, ProviderMessageId,
    ProviderThreadId, SystemLabel, Timestamp,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, ResponseTemplate};

fn id(s: &str) -> ProviderMessageId {
    ProviderMessageId(s.into())
}

/// The fixture, with Inbox, Trash and Receipts, and two unread messages
/// in the inbox.
async fn with_mail() -> Fixture {
    let f = fixture().await;
    for (pid, name, kind) in [
        ("INBOX", "INBOX", LabelKind::System(SystemLabel::Inbox)),
        ("TRASH", "TRASH", LabelKind::System(SystemLabel::Trash)),
        ("Label_1", "Receipts", LabelKind::User),
    ] {
        f.store
            .put_label(&f.me, &Label::new(pid, name, kind))
            .unwrap();
    }
    for m in ["m1", "m2"] {
        let message = Message {
            account: f.me.clone(),
            id: id(m),
            thread: ProviderThreadId(format!("t-{m}")),
            headers: MessageHeaders::default(),
            received: Timestamp::from_millis(1),
            labels: vec!["INBOX".into()],
            flags: Flags {
                unread: true,
                starred: false,
            },
            snippet: String::new(),
            body_text: String::new(),
            attachments: Vec::new(),
            size: 1,
            downloaded: true,
        };
        f.store.put_message(&message, None).unwrap();
    }
    f
}

async fn answer(f: &Fixture, p: &str, status: u16) {
    Mock::given(method("POST"))
        .and(path_regex(p))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({})))
        .mount(&f.server)
        .await;
}

/// The bodies of the POSTs to `p`, in order.
async fn posted(f: &Fixture, p: &str) -> Vec<Value> {
    f.server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == p)
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

#[tokio::test]
async fn changes_reach_gmail_in_order_as_its_calls() {
    let f = with_mail().await;
    answer(&f, "^/gmail/messages/batchModify$", 204).await;
    answer(&f, "^/gmail/messages/.*/trash$", 200).await;
    let both = [id("m1"), id("m2")];
    let mut archive_read = MessageChange::remove_label("INBOX");
    archive_read.unread = Some(false);
    f.store
        .change_messages(&f.me, &both, &archive_read)
        .unwrap();
    f.store
        .change_messages(&f.me, &[id("m1")], &MessageChange::starred(true))
        .unwrap();
    f.store
        .change_messages(&f.me, &[id("m2")], &MessageChange::add_label("TRASH"))
        .unwrap();

    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 3);
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!(report.sent, 3);
    assert!(report.refused.is_empty());
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 0);

    assert_eq!(
        posted(&f, "/gmail/messages/batchModify").await,
        [
            json!({"ids": ["m1", "m2"], "addLabelIds": [], "removeLabelIds": ["INBOX", "UNREAD"]}),
            json!({"ids": ["m1"], "addLabelIds": ["STARRED"], "removeLabelIds": []}),
        ]
    );
    assert_eq!(posted(&f, "/gmail/messages/m2/trash").await.len(), 1);
    // Sent entries stay until a sync has passed them.
    assert!(
        f.store
            .outbox(&f.me)
            .unwrap()
            .iter()
            .all(|e| e.sent.is_some())
    );
}

#[tokio::test]
async fn a_refused_change_is_dropped_and_gmail_read_again() {
    let f = with_mail().await;
    mock_labels(&f.server).await;
    answer(&f, "^/gmail/messages/batchModify$", 400).await;
    // Gmail still has m1 unread in the inbox.
    get(
        &f.server,
        "messages/m1",
        json!({"id": "m1", "threadId": "t-m1", "labelIds": ["INBOX", "UNREAD"]}),
    )
    .await;
    f.store
        .change_messages(&f.me, &[id("m1")], &MessageChange::add_label("Label_1"))
        .unwrap();
    assert!(
        f.store
            .message(&f.me, &id("m1"))
            .unwrap()
            .unwrap()
            .labels
            .contains(&"Label_1".into())
    );

    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (0, 1));
    assert!(f.store.outbox(&f.me).unwrap().is_empty());
    let m1 = f.store.message(&f.me, &id("m1")).unwrap().unwrap();
    assert_eq!(m1.labels, ["INBOX"], "back to what Gmail has");
}

#[tokio::test]
async fn a_failure_that_may_pass_holds_the_rest_back() {
    let f = with_mail().await;
    // Forbidden, without a rate-limit reason: not retried, not final.
    answer(&f, "^/gmail/messages/batchModify$", 403).await;
    f.store
        .change_messages(&f.me, &[id("m1")], &MessageChange::read())
        .unwrap();
    f.store
        .change_messages(&f.me, &[id("m2")], &MessageChange::read())
        .unwrap();

    assert!(f.sync.send_outbox(&f.me).await.is_err());
    let outbox = f.store.outbox(&f.me).unwrap();
    assert_eq!(outbox.len(), 2, "both kept");
    assert_eq!(outbox[0].attempts, 1);
    assert!(outbox[0].error.is_some());
    assert_eq!(
        outbox[1].attempts, 0,
        "the second was not tried out of order"
    );
    assert_eq!(posted(&f, "/gmail/messages/batchModify").await.len(), 1);
    // Still applied here meanwhile.
    assert!(
        !f.store
            .message(&f.me, &id("m2"))
            .unwrap()
            .unwrap()
            .flags
            .unread
    );
}

#[tokio::test]
async fn a_finished_sync_forgets_what_was_sent_before_it() {
    let f = with_mail().await;
    answer(&f, "^/gmail/messages/batchModify$", 204).await;
    f.store
        .change_messages(&f.me, &[id("m1")], &MessageChange::read())
        .unwrap();
    f.sync.send_outbox(&f.me).await.unwrap();

    // A sync with nothing new to say.
    f.store
        .set_sync_cursor(&f.me, Some(r#"{"kind":"history","history_id":"100"}"#))
        .unwrap();
    mock_history(&f.server, "100", json!({"historyId": "101"})).await;
    f.sync.sync(&f.me, quiet()).await.unwrap();
    assert!(f.store.outbox(&f.me).unwrap().is_empty());
}
