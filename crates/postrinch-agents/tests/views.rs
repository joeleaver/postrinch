//! An agent's views calls, and its changes to single messages, over a real
//! loopback connection.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use postrinch_agents::client::Client;
use postrinch_agents::view_shape::ViewShape;
use postrinch_agents::{Agents, Call, Changed, Reach};
use postrinch_commands::{Commands, ThreadAction};
use postrinch_model::{
    Account, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders, Provider,
    ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};
use postrinch_store::Store;
use serde_json::Value;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

fn message(account: &EmailAddress, id: &str, at: i64) -> Message {
    Message {
        account: account.clone(),
        id: ProviderMessageId(id.into()),
        thread: ProviderThreadId("t1".into()),
        headers: MessageHeaders {
            subject: Some("Thursday rota".into()),
            from: vec![Mailbox::new(Some("Ada".into()), address("ada@example.org"))],
            ..Default::default()
        },
        received: Timestamp::from_millis(at),
        labels: vec!["INBOX".into()],
        flags: Flags {
            unread: true,
            starred: false,
        },
        snippet: "who can cover".into(),
        body_text: String::new(),
        downloaded: true,
        attachments: Vec::new(),
        size: 0,
    }
}

fn add_account(store: &Store, account: &EmailAddress) {
    store
        .put_account(&Account::new(account.clone(), Provider::Google))
        .unwrap();
    store
        .put_label(
            account,
            &Label::new("INBOX", "INBOX", LabelKind::System(SystemLabel::Inbox)),
        )
        .unwrap();
    for (id, at) in [("m1", 1_762_200_000_000), ("m2", 1_762_200_100_000)] {
        store.put_message(&message(account, id, at), None).unwrap();
    }
}

fn change(json: &str) -> ViewShape {
    serde_json::from_str(json).unwrap()
}

fn find<'a>(views: &'a Value, key: &str) -> Option<&'a Value> {
    views.as_array().unwrap().iter().find(|v| v["view"] == key)
}

#[test]
fn an_agent_sees_and_changes_views_of_its_own_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("store")).unwrap();
    let work = address("work@example.org");
    let home = address("home@example.org");
    add_account(&store, &work);
    add_account(&store, &home);

    let agents_path = dir.path().join("agents.json");
    let mut agents = Agents::load(&agents_path).unwrap();
    let (_, key) = agents
        .create("Test agent", Reach::Only(vec![work.clone()]))
        .unwrap();
    let changes = Arc::new(Mutex::new(Vec::new()));
    let seen = changes.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let commands = Commands::new(store.clone());
    postrinch_agents::serve::start_on(
        listener,
        commands.clone(),
        agents_path,
        postrinch_agents::serve::Hooks {
            on_change: Arc::new(move |c: &Changed| seen.lock().unwrap().push(c.clone())),
            gmail: None,
        },
    )
    .unwrap();
    let client = Client::new(addr, Some(key));

    // The person's view of their other account is not there for it.
    let mut theirs = commands.new_view("Home", "is:unread").unwrap();
    theirs.accounts = vec![home.clone()];
    commands.save_view(&theirs).unwrap();
    let views = client.call(&Call::Views).unwrap();
    assert!(find(&views, &theirs.key).is_none(), "{views}");
    let inbox = find(&views, "inbox").unwrap();
    assert_eq!(inbox["kind"], "built_in");
    assert_eq!(inbox["may_change"], false);
    assert_eq!(inbox["conversations"], 1, "only its account's: {inbox}");

    // A view of every account is the person's to change.
    let refused = client
        .call(&Call::SaveView {
            view: Some("Inbox".into()),
            change: change(r#"{"order": "oldest_first"}"#),
        })
        .unwrap_err();
    assert!(refused.contains("accounts"), "{refused}");

    // A new one is of its own accounts.
    let made = client
        .call(&Call::SaveView {
            view: None,
            change: change(r#"{"name": "Rota", "search": "subject:rota"}"#),
        })
        .unwrap();
    assert_eq!(made["accounts"], serde_json::json!([work]));
    assert_eq!(
        (made["kind"].as_str(), made["conversations"].as_u64()),
        (Some("yours"), Some(1))
    );
    assert_eq!(changes.lock().unwrap().as_slice(), [Changed::Views]);

    let listed = client
        .call(&Call::ListView {
            view: "rota".into(),
            offset: 0,
            limit: 10,
        })
        .unwrap();
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["threads"][0]["thread"], "t1");

    let changed = client
        .call(&Call::SaveView {
            view: Some("Rota".into()),
            change: change(r#"{"rows": "messages", "sidebar": "when_it_has_mail"}"#),
        })
        .unwrap();
    assert_eq!(
        (changed["rows"].as_str(), changed["sidebar"].as_str()),
        (Some("messages"), Some("when_it_has_mail"))
    );
    assert_eq!(changed["search"], "subject:rota", "kept");

    let deleted = client
        .call(&Call::DeleteView {
            view: "Rota".into(),
        })
        .unwrap();
    assert_eq!(deleted["was"]["name"], "Rota");
    assert!(
        client
            .call(&Call::Views)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["name"] != "Rota")
    );
}

#[test]
fn an_agent_changes_one_message_of_a_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("store")).unwrap();
    let work = address("work@example.org");
    add_account(&store, &work);
    let agents_path = dir.path().join("agents.json");
    let mut agents = Agents::load(&agents_path).unwrap();
    let (_, key) = agents.create("Test agent", Reach::Every).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    postrinch_agents::serve::start_on(
        listener,
        Commands::new(store.clone()),
        agents_path,
        postrinch_agents::serve::Hooks {
            on_change: Arc::new(|_: &Changed| {}),
            gmail: None,
        },
    )
    .unwrap();
    let client = Client::new(addr, Some(key));

    let answer = client
        .call(&Call::Act {
            threads: Vec::new(),
            search: None,
            messages: vec![(work.clone(), ProviderMessageId("m1".into()))],
            actions: vec![ThreadAction::Archive],
        })
        .unwrap();
    assert_eq!(answer["changed_messages"], 1);
    let labels = |id: &str| {
        store
            .message(&work, &ProviderMessageId(id.into()))
            .unwrap()
            .unwrap()
            .labels
    };
    assert!(!labels("m1").contains(&"INBOX".to_owned()));
    assert!(labels("m2").contains(&"INBOX".to_owned()), "the rest stays");
}
