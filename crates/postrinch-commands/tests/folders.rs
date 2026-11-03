use postrinch_commands::{Commands, Folder};
use postrinch_model::{
    Account, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders, Provider,
    ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};
use postrinch_store::Store;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

fn account(store: &Store, me: &EmailAddress) {
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    for (id, name, kind) in [
        ("INBOX", "Inbox", LabelKind::System(SystemLabel::Inbox)),
        ("SENT", "Sent", LabelKind::System(SystemLabel::Sent)),
        (
            "CATEGORY_UPDATES",
            "CATEGORY_UPDATES",
            LabelKind::System(SystemLabel::CategoryUpdates),
        ),
        ("Label_1", "Receipts", LabelKind::User),
    ] {
        let label = Label::new(id, name, kind);
        store.put_label(me, &label).unwrap();
    }
}

fn message(
    me: &EmailAddress,
    id: &str,
    thread: &str,
    at: i64,
    labels: &[&str],
    unread: bool,
) -> Message {
    Message {
        account: me.clone(),
        id: ProviderMessageId(id.into()),
        thread: ProviderThreadId(thread.into()),
        headers: MessageHeaders {
            subject: Some(format!("about {thread}")),
            from: vec![Mailbox::new(None, address("ada@example.org"))],
            to: vec![Mailbox::new(None, me.clone())],
            ..Default::default()
        },
        received: Timestamp::from_millis(at),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        flags: Flags {
            unread,
            starred: false,
        },
        snippet: String::new(),
        body_text: String::new(),
        downloaded: false,
        attachments: Vec::new(),
        size: 0,
    }
}

fn setup() -> (tempfile::TempDir, Commands, EmailAddress, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (a, b) = (address("a@example.org"), address("b@example.org"));
    account(&store, &a);
    account(&store, &b);
    for m in [
        message(&a, "a1", "ta1", 100, &["INBOX"], true),
        message(&a, "a2", "ta2", 300, &["INBOX", "Label_1"], false),
        message(&a, "a3", "ta2", 350, &["SENT"], false),
        message(&b, "b1", "tb1", 200, &["INBOX", "CATEGORY_UPDATES"], true),
    ] {
        store.put_message(&m, None).unwrap();
    }
    (dir, Commands::new(store), a, b)
}

#[test]
fn sidebar_unifies_inboxes_and_lists_each_accounts_labels() {
    let (_dir, commands, a, _b) = setup();
    let sidebar = commands.sidebar().unwrap();
    let everywhere: Vec<_> = sidebar
        .everywhere
        .iter()
        .map(|f| (f.name.as_str(), f.unread))
        .collect();
    assert_eq!(everywhere, [("All inboxes", 2), ("Sent", 0), ("Drafts", 0)]);
    assert_eq!(sidebar.accounts.len(), 2);
    let first = &sidebar.accounts[0];
    assert_eq!(first.account.address, a);
    assert_eq!(first.inbox.unread, 1);
    let labels: Vec<_> = first.labels.own.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(labels, ["Receipts"]);
}

#[test]
fn all_inboxes_merge_accounts_by_latest_activity_and_page() {
    let (_dir, commands, _a, _b) = setup();
    let folder = Folder::Everywhere(SystemLabel::Inbox);
    let page = commands.list_threads(&folder, None, 0, 10).unwrap();
    let ids: Vec<_> = page
        .threads
        .iter()
        .map(|t| t.summary.id.0.as_str())
        .collect();
    // ta2's latest message is the reply in Sent, at 350.
    assert_eq!(ids, ["ta2", "tb1", "ta1"]);
    assert_eq!(page.total, 3);
    assert_eq!(page.threads[0].summary.message_count, 2);

    let second = commands.list_threads(&folder, None, 1, 1).unwrap();
    assert_eq!(second.threads[0].summary.id.0, "tb1");
    assert_eq!(second.total, 3);
}

#[test]
fn reads_a_thread_and_refuses_unknown_accounts() {
    let (_dir, commands, a, _b) = setup();
    let thread = commands
        .read_thread(&a, &ProviderThreadId("ta2".into()))
        .unwrap()
        .unwrap();
    assert_eq!(thread.messages.len(), 2);
    assert!(
        commands
            .read_thread(&a, &ProviderThreadId("nope".into()))
            .unwrap()
            .is_none()
    );

    let stranger = Folder::Label {
        account: address("c@example.org"),
        label: "INBOX".into(),
    };
    assert!(commands.list_threads(&stranger, None, 0, 10).is_err());
}

#[test]
fn a_category_narrows_the_inbox_like_gmails_tabs() {
    let (_dir, commands, _a, _b) = setup();
    let inbox = Folder::Everywhere(SystemLabel::Inbox);
    let updates = commands
        .list_threads(&inbox, Some(SystemLabel::CategoryUpdates), 0, 10)
        .unwrap();
    let ids: Vec<_> = updates
        .threads
        .iter()
        .map(|t| t.summary.id.0.as_str())
        .collect();
    assert_eq!(ids, ["tb1"]);
    assert_eq!(updates.total, 1);
}

#[test]
fn rows_carry_tag_like_labels_but_not_the_one_listed() {
    let (_dir, commands, a, _b) = setup();
    let inbox = Folder::Everywhere(SystemLabel::Inbox);
    let page = commands.list_threads(&inbox, None, 0, 10).unwrap();
    let ta2 = page
        .threads
        .iter()
        .find(|t| t.summary.id.0 == "ta2")
        .unwrap();
    // Receipts' only message is in the inbox: a tag, so a chip.
    let chips: Vec<_> = ta2.chips.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(chips, ["Receipts"]);

    let receipts = Folder::Label {
        account: a,
        label: "Label_1".into(),
    };
    let page = commands.list_threads(&receipts, None, 0, 10).unwrap();
    assert!(page.threads[0].chips.is_empty());
}

#[test]
fn label_prefs_rename_hide_and_pin_here_only() {
    let (_dir, commands, a, _b) = setup();
    let prefs = postrinch_model::LabelPrefs {
        name: Some("  Bills  ".into()),
        pinned: true,
        ..Default::default()
    };
    commands.set_label_prefs(&a, "Label_1", &prefs).unwrap();
    assert_eq!(
        commands.label_prefs(&a, "Label_1").unwrap().name.as_deref(),
        Some("Bills")
    );
    let sidebar = commands.sidebar().unwrap();
    assert_eq!(sidebar.accounts[0].labels.pinned[0].name, "Bills");

    let bad = postrinch_model::LabelPrefs {
        color: Some("blue".into()),
        ..Default::default()
    };
    assert!(commands.set_label_prefs(&a, "Label_1", &bad).is_err());
    let blank = postrinch_model::LabelPrefs {
        name: Some("   ".into()),
        ..Default::default()
    };
    commands.set_label_prefs(&a, "Label_1", &blank).unwrap();
    assert_eq!(commands.label_prefs(&a, "Label_1").unwrap().name, None);
}
