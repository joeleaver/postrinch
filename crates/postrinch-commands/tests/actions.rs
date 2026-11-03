//! Changing mail through the commands: what each action does to a thread.

use postrinch_commands::{Commands, Folder, ThreadAction};
use postrinch_model::{
    Account, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders, Provider,
    ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};
use postrinch_store::Store;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

fn message(me: &EmailAddress, id: &str, at: i64, labels: &[&str], flags: Flags) -> Message {
    Message {
        account: me.clone(),
        id: ProviderMessageId(id.into()),
        thread: ProviderThreadId("t1".into()),
        headers: MessageHeaders {
            subject: Some("Thursday rota".into()),
            from: vec![Mailbox::new(None, address("ada@example.org"))],
            to: vec![Mailbox::new(None, me.clone())],
            ..Default::default()
        },
        received: Timestamp::from_millis(at),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        flags,
        snippet: String::new(),
        body_text: "who can cover thursday".into(),
        downloaded: true,
        attachments: Vec::new(),
        size: 0,
    }
}

const UNREAD: Flags = Flags {
    unread: true,
    starred: false,
};
const READ: Flags = Flags {
    unread: false,
    starred: false,
};

/// One thread of three messages: m1 read in the inbox and Receipts, m2
/// unread in the inbox, m3 (latest) unread and only in Sent.
fn setup() -> (tempfile::TempDir, Store, Commands, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = address("me@example.org");
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    for (id, kind) in [
        ("INBOX", LabelKind::System(SystemLabel::Inbox)),
        ("SENT", LabelKind::System(SystemLabel::Sent)),
        ("TRASH", LabelKind::System(SystemLabel::Trash)),
        ("Label_1", LabelKind::User),
    ] {
        store.put_label(&me, &Label::new(id, id, kind)).unwrap();
    }
    for m in [
        message(&me, "m1", 100, &["INBOX", "Label_1"], READ),
        message(&me, "m2", 200, &["INBOX"], UNREAD),
        message(&me, "m3", 300, &["SENT"], UNREAD),
    ] {
        store.put_message(&m, None).unwrap();
    }
    let commands = Commands::new(store.clone());
    (dir, store, commands, me)
}

fn thread(me: &EmailAddress) -> [(EmailAddress, ProviderThreadId); 1] {
    [(me.clone(), ProviderThreadId("t1".into()))]
}

/// Each message's labels and flags, oldest first.
fn state(store: &Store, me: &EmailAddress) -> Vec<(String, Vec<String>, Flags)> {
    store
        .thread_messages(me, &ProviderThreadId("t1".into()))
        .unwrap()
        .into_iter()
        .map(|m| (m.id.0, m.labels, m.flags))
        .collect()
}

/// The messages each outbox entry names.
fn sent(store: &Store, me: &EmailAddress) -> Vec<Vec<String>> {
    store
        .outbox(me)
        .unwrap()
        .into_iter()
        .map(|e| e.messages.into_iter().map(|m| m.0).collect())
        .collect()
}

#[test]
fn marking_read_changes_only_unread_messages_and_only_once() {
    let (_dir, store, commands, me) = setup();
    commands.act(&thread(&me), &ThreadAction::MarkRead).unwrap();
    commands.act(&thread(&me), &ThreadAction::MarkRead).unwrap();
    assert_eq!(sent(&store, &me), [vec!["m2", "m3"]]);
    assert!(state(&store, &me).iter().all(|(_, _, f)| !f.unread));
}

#[test]
fn unread_and_star_go_to_the_latest_message() {
    let (_dir, store, commands, me) = setup();
    commands.act(&thread(&me), &ThreadAction::MarkRead).unwrap();
    commands
        .act(&thread(&me), &ThreadAction::MarkUnread)
        .unwrap();
    commands.act(&thread(&me), &ThreadAction::Star).unwrap();
    let flags: Vec<Flags> = state(&store, &me).into_iter().map(|(_, _, f)| f).collect();
    assert_eq!(
        flags,
        [
            READ,
            READ,
            Flags {
                unread: true,
                starred: true
            }
        ]
    );
    commands.act(&thread(&me), &ThreadAction::Unstar).unwrap();
    assert!(state(&store, &me).iter().all(|(_, _, f)| !f.starred));
}

#[test]
fn archiving_leaves_the_inbox_and_keeps_every_other_label() {
    let (_dir, store, commands, me) = setup();
    let inbox = Folder::Everywhere(SystemLabel::Inbox);
    assert_eq!(commands.list_threads(&inbox, None, 0, 10).unwrap().total, 1);

    commands.act(&thread(&me), &ThreadAction::Archive).unwrap();
    assert_eq!(
        sent(&store, &me),
        [vec!["m1", "m2"]],
        "only those in the inbox"
    );
    let labels: Vec<Vec<String>> = state(&store, &me).into_iter().map(|(_, l, _)| l).collect();
    assert_eq!(labels, [vec!["Label_1"], vec![], vec!["SENT"]]);
    assert_eq!(commands.list_threads(&inbox, None, 0, 10).unwrap().total, 0);

    commands
        .act(&thread(&me), &ThreadAction::MoveToInbox)
        .unwrap();
    assert_eq!(commands.list_threads(&inbox, None, 0, 10).unwrap().total, 1);
}

#[test]
fn trashed_threads_leave_the_inbox_and_search_until_untrashed() {
    let (_dir, _store, commands, me) = setup();
    let inbox = Folder::Everywhere(SystemLabel::Inbox);
    assert_eq!(commands.search("thursday", 10).unwrap().threads.len(), 1);

    commands.act(&thread(&me), &ThreadAction::Trash).unwrap();
    assert_eq!(commands.list_threads(&inbox, None, 0, 10).unwrap().total, 0);
    assert!(commands.search("thursday", 10).unwrap().threads.is_empty());

    commands.act(&thread(&me), &ThreadAction::Untrash).unwrap();
    assert_eq!(commands.list_threads(&inbox, None, 0, 10).unwrap().total, 1);
    assert_eq!(commands.search("thursday", 10).unwrap().threads.len(), 1);
}

#[test]
fn labels_are_added_where_missing_and_removed_where_present() {
    let (_dir, store, commands, me) = setup();
    commands
        .act(&thread(&me), &ThreadAction::AddLabel("Label_1".into()))
        .unwrap();
    assert_eq!(sent(&store, &me), [vec!["m2", "m3"]]);
    commands
        .act(&thread(&me), &ThreadAction::RemoveLabel("Label_1".into()))
        .unwrap();
    assert_eq!(sent(&store, &me)[1], ["m1", "m2", "m3"]);
    assert!(
        state(&store, &me)
            .iter()
            .all(|(_, l, _)| !l.contains(&"Label_1".to_owned()))
    );
}

#[test]
fn actions_serialize_for_agents() {
    let json = serde_json::to_value(ThreadAction::AddLabel("Label_1".into())).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"action": "add_label", "label": "Label_1"})
    );
    let archive: ThreadAction = serde_json::from_str(r#"{"action": "archive"}"#).unwrap();
    assert_eq!(archive, ThreadAction::Archive);
}
