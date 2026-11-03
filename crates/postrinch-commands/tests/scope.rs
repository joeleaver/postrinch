//! An agent limited to some accounts: to it, the others do not exist.

use postrinch_commands::{CommandError, Commands, Folder, ThreadAction};
use postrinch_model::{
    Account, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders, Provider,
    ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};
use postrinch_store::Store;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

/// One unread inbox message about the rota in `account`.
fn add_account(store: &Store, account: &EmailAddress) {
    store
        .put_account(&Account::new(account.clone(), Provider::Google))
        .unwrap();
    let inbox = Label::new("INBOX", "INBOX", LabelKind::System(SystemLabel::Inbox));
    store.put_label(account, &inbox).unwrap();
    let message = Message {
        account: account.clone(),
        id: ProviderMessageId("m1".into()),
        thread: ProviderThreadId("t1".into()),
        headers: MessageHeaders {
            subject: Some("Thursday rota".into()),
            from: vec![Mailbox::new(None, address("ada@example.org"))],
            ..Default::default()
        },
        received: Timestamp::from_millis(100),
        labels: vec!["INBOX".into()],
        flags: Flags {
            unread: true,
            starred: false,
        },
        snippet: String::new(),
        body_text: "who can cover thursday".into(),
        downloaded: true,
        attachments: Vec::new(),
        size: 0,
    };
    store.put_message(&message, None).unwrap();
}

fn hidden<T>(r: Result<T, CommandError>) -> bool {
    matches!(r, Err(CommandError::UnknownAccount(_)))
}

#[test]
fn an_agent_sees_only_its_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let work = address("work@example.org");
    let home = address("home@example.org");
    add_account(&store, &work);
    add_account(&store, &home);
    let all = Commands::new(store.clone());
    let agent = all.limited_to(std::slice::from_ref(&work));

    let addresses = |c: &Commands| -> Vec<String> {
        c.accounts()
            .unwrap()
            .into_iter()
            .map(|a| a.account.address.to_string())
            .collect()
    };
    assert_eq!(addresses(&all).len(), 2);
    assert_eq!(addresses(&agent), ["work@example.org"]);

    let sidebar = agent.sidebar().unwrap();
    assert_eq!(sidebar.accounts.len(), 1);
    assert_eq!(sidebar.everywhere[0].unread, 1, "only work's unread counts");

    let inbox = Folder::Everywhere(SystemLabel::Inbox);
    let listed = agent.list_threads(&inbox, None, 0, 10).unwrap();
    assert_eq!(listed.total, 1);
    assert_eq!(listed.threads[0].summary.account, work);

    let found = agent.search("thursday", 10).unwrap();
    assert_eq!(found.threads.len(), 1);
    assert_eq!(found.threads[0].summary.account, work);
    assert_eq!(all.search("thursday", 10).unwrap().threads.len(), 2);

    let t1 = ProviderThreadId("t1".into());
    assert!(agent.read_thread(&work, &t1).unwrap().is_some());
    assert!(hidden(agent.read_thread(&home, &t1)));
    assert!(hidden(agent.label_choices(&home)));
    let home_inbox = Folder::Label {
        account: home.clone(),
        label: "INBOX".into(),
    };
    assert!(hidden(agent.list_threads(&home_inbox, None, 0, 10)));
    let widened = agent.limited_to(&[work.clone(), home.clone()]);
    assert_eq!(
        addresses(&widened),
        ["work@example.org"],
        "limiting never widens"
    );

    // Acting on a thread it cannot see changes nothing, even beside one it can.
    let both = [(work.clone(), t1.clone()), (home.clone(), t1.clone())];
    assert!(hidden(agent.act(&both, &ThreadAction::MarkRead)));
    for account in [&work, &home] {
        let messages = store.thread_messages(account, &t1).unwrap();
        assert!(messages[0].flags.unread, "{account} untouched");
    }
}
