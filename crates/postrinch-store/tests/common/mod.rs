//! Helpers shared by the store's integration tests.

use postrinch_model::{
    Account, AttachmentInfo, EmailAddress, Flags, Label, LabelKind, Mailbox, Message,
    MessageHeaders, Provider, ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};
use postrinch_store::Store;

pub fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

pub fn setup() -> (tempfile::TempDir, Store, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = address("me@example.org");
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    for (id, name, kind) in [
        ("INBOX", "Inbox", LabelKind::System(SystemLabel::Inbox)),
        ("Label_1", "Receipts", LabelKind::User),
    ] {
        let label = Label {
            provider_id: id.into(),
            name: name.into(),
            kind,
        };
        store.put_label(&me, &label).unwrap();
    }
    (dir, store, me)
}

pub fn message(
    account: &EmailAddress,
    id: &str,
    thread: &str,
    received: i64,
    subject: &str,
    body: &str,
) -> Message {
    Message {
        account: account.clone(),
        id: ProviderMessageId(id.into()),
        thread: ProviderThreadId(thread.into()),
        headers: MessageHeaders {
            message_id: Some(format!("{id}@mail.example.org")),
            subject: Some(subject.into()),
            from: vec![Mailbox::new(
                Some("Ada Lovelace".into()),
                address("ada@example.org"),
            )],
            to: vec![Mailbox::new(None, account.clone())],
            ..Default::default()
        },
        received: Timestamp::from_millis(received),
        labels: vec!["INBOX".into()],
        flags: Flags {
            unread: true,
            starred: false,
        },
        snippet: body.chars().take(40).collect(),
        body_text: body.into(),
        downloaded: true,
        attachments: vec![AttachmentInfo {
            filename: Some("notes.pdf".into()),
            mime_type: "application/pdf".into(),
            size: 1234,
            content_id: None,
            inline: false,
        }],
        size: 4096,
    }
}
