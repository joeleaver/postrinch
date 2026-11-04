//! Searching with the search language: words, Gmail's operators, and
//! what is left for Gmail to search.

use std::collections::HashSet;

use postrinch_commands::Commands;
use postrinch_model::{
    Account, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders, Provider,
    ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};
use postrinch_store::Store;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

struct Mail<'a> {
    id: &'a str,
    thread: &'a str,
    at: i64,
    from: &'a str,
    subject: &'a str,
    body: Option<&'a str>,
    labels: &'a [&'a str],
    unread: bool,
}

fn message(me: &EmailAddress, m: &Mail) -> Message {
    Message {
        account: me.clone(),
        id: ProviderMessageId(m.id.into()),
        thread: ProviderThreadId(m.thread.into()),
        headers: MessageHeaders {
            subject: Some(m.subject.into()),
            from: vec![Mailbox::new(None, address(m.from))],
            to: vec![Mailbox::new(None, me.clone())],
            ..Default::default()
        },
        received: Timestamp::from_millis(m.at),
        labels: m.labels.iter().map(|l| l.to_string()).collect(),
        flags: Flags {
            unread: m.unread,
            starred: false,
        },
        snippet: String::new(),
        body_text: m.body.unwrap_or_default().into(),
        downloaded: m.body.is_some(),
        attachments: Vec::new(),
        size: 0,
    }
}

fn setup() -> (tempfile::TempDir, Commands, EmailAddress, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (a, b) = (address("a@example.org"), address("b@other.org"));
    for me in [&a, &b] {
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();
        for (id, name, kind) in [
            ("INBOX", "Inbox", LabelKind::System(SystemLabel::Inbox)),
            ("TRASH", "Trash", LabelKind::System(SystemLabel::Trash)),
            ("Label_1", "Work/Clients", LabelKind::User),
        ] {
            store.put_label(me, &Label::new(id, name, kind)).unwrap();
        }
    }
    let mail = [
        (
            &a,
            Mail {
                id: "a1",
                thread: "t1",
                at: 100,
                from: "mary@rota.org",
                subject: "Can someone cover my shift?",
                body: Some("Thursday is free"),
                labels: &["INBOX", "Label_1"],
                unread: false,
            },
        ),
        (
            &a,
            Mail {
                id: "a2",
                thread: "t1",
                at: 200,
                from: "michael@rota.org",
                subject: "Re: Can someone cover my shift?",
                body: Some("I can take Thursday"),
                labels: &["INBOX", "Label_1"],
                unread: true,
            },
        ),
        (
            &a,
            Mail {
                id: "a3",
                thread: "t2",
                at: 300,
                from: "shop@store.com",
                subject: "Your order",
                body: Some("Thursday delivery"),
                labels: &["INBOX"],
                unread: false,
            },
        ),
        (
            &a,
            Mail {
                id: "a4",
                thread: "t3",
                at: 400,
                from: "spam@x.com",
                subject: "Thursday deal",
                body: Some("deal"),
                labels: &["TRASH"],
                unread: false,
            },
        ),
        (
            &b,
            Mail {
                id: "b1",
                thread: "u1",
                at: 250,
                from: "mary@rota.org",
                subject: "Old rota",
                body: None,
                labels: &["INBOX"],
                unread: false,
            },
        ),
    ];
    for (me, m) in &mail {
        store.put_message(&message(me, m), None).unwrap();
    }
    (dir, Commands::new(store), a, b)
}

fn threads(commands: &Commands, query: &str) -> Vec<String> {
    commands
        .search(query, 10)
        .unwrap()
        .threads
        .iter()
        .map(|r| r.summary.id.0.clone())
        .collect()
}

#[test]
fn every_word_must_match_and_newest_comes_first() {
    let (_dir, commands, _, _) = setup();
    assert_eq!(
        threads(&commands, "thursday"),
        ["t2", "t1"],
        "the trash is left out"
    );
    assert_eq!(threads(&commands, "thursday take"), ["t1"]);
    assert_eq!(
        threads(&commands, "thursday in:anywhere"),
        ["t3", "t2", "t1"]
    );
    assert_eq!(threads(&commands, "deal in:trash"), ["t3"]);
}

#[test]
fn operators_narrow() {
    let (_dir, commands, _, _) = setup();
    assert_eq!(threads(&commands, "from:rota"), ["u1", "t1"]);
    assert_eq!(threads(&commands, "from:rota account:a@"), ["t1"]);
    assert_eq!(threads(&commands, "label:work-clients is:unread"), ["t1"]);
    assert_eq!(
        threads(&commands, "-label:work-clients in:inbox"),
        ["t2", "u1"]
    );
    assert_eq!(threads(&commands, "subject:order"), ["t2"]);
    assert_eq!(
        threads(&commands, "after:1969/12/30 before:1970/01/03 from:shop"),
        ["t2"]
    );
    assert!(commands.search("label:nowhere", 10).is_err());
}

#[test]
fn gmail_is_asked_only_for_words_in_mail_not_downloaded() {
    let (_dir, commands, a, b) = setup();
    let results = commands.search("rota account:b@", 10).unwrap();
    assert_eq!(results.threads.len(), 1, "matched on its sender's address");
    let remote = results.remote.unwrap();
    assert_eq!(remote.query, "rota");
    assert_eq!(remote.accounts, [(b.clone(), 1)]);
    assert!(
        commands.search("from:rota", 10).unwrap().remote.is_none(),
        "operators alone are answered here"
    );
    assert!(
        commands
            .search("thursday account:a@", 10)
            .unwrap()
            .remote
            .is_none()
    );

    // What Gmail found: listed threads and the trash left out, the rest marked.
    let listed: HashSet<ProviderThreadId> = [ProviderThreadId("t1".into())].into();
    let ids: Vec<ProviderMessageId> = ["a2", "a3", "a4"]
        .iter()
        .map(|s| ProviderMessageId((*s).into()))
        .collect();
    let rows = commands.remote_rows("thursday", &a, &ids, &listed).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].summary.id.0, "t2");
    assert!(rows[0].remote);
}
