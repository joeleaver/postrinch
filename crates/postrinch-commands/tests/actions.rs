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

#[test]
fn one_message_at_a_time_lists_each_inbox_message_newest_first() {
    let (_dir, _store, commands, _me) = setup();
    let inbox = Folder::Everywhere(SystemLabel::Inbox);
    let page = commands.list_messages(&inbox, None, 0, 10).unwrap();
    assert_eq!(page.total, 2, "m3 is only in Sent");
    let ids: Vec<&str> = page
        .threads
        .iter()
        .map(|r| r.message.as_ref().unwrap().0.as_str())
        .collect();
    assert_eq!(ids, ["m2", "m1"]);
    assert_eq!(page.threads[0].summary.unread_count, 1);
    assert_eq!(page.threads[1].summary.unread_count, 0);
    assert_eq!(
        commands
            .list_messages(&inbox, None, 1, 10)
            .unwrap()
            .threads
            .len(),
        1
    );
}

#[test]
fn archiving_one_message_leaves_the_rest_of_its_conversation() {
    let (_dir, store, commands, me) = setup();
    let inbox = Folder::Everywhere(SystemLabel::Inbox);
    let m2 = [(me.clone(), ProviderMessageId("m2".into()))];
    commands.act_messages(&m2, &ThreadAction::Archive).unwrap();
    assert_eq!(sent(&store, &me), [vec!["m2"]]);
    let page = commands.list_messages(&inbox, None, 0, 10).unwrap();
    assert_eq!(page.threads.len(), 1);
    assert_eq!(page.threads[0].message.as_ref().unwrap().0, "m1");
    assert_eq!(commands.list_threads(&inbox, None, 0, 10).unwrap().total, 1);

    // Unread and star go to the message named, not the latest.
    let m1 = [(me.clone(), ProviderMessageId("m1".into()))];
    commands
        .act_messages(&m1, &ThreadAction::MarkUnread)
        .unwrap();
    commands.act_messages(&m1, &ThreadAction::Star).unwrap();
    let flags: Vec<Flags> = state(&store, &me).into_iter().map(|(_, _, f)| f).collect();
    assert_eq!(
        flags,
        [
            Flags {
                unread: true,
                starred: true
            },
            UNREAD,
            UNREAD
        ]
    );
}

#[test]
fn search_by_message_lists_each_match() {
    let (_dir, _store, commands, _me) = setup();
    let by_thread = commands.search("thursday", 10).unwrap();
    assert_eq!(by_thread.threads.len(), 1);
    let by_message = commands
        .search_grouped("thursday", 10, postrinch_commands::Grouping::Messages)
        .unwrap();
    assert_eq!(by_message.threads.len(), 3);
    assert!(by_message.threads.iter().all(|r| r.message.is_some()));
}
