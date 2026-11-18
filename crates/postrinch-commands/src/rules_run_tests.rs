use postrinch_model::{
    Account, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders, Provider,
    ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};
use postrinch_rules::gmail::Filter;
use postrinch_rules::{Action, Condition, Made, Rule, Test, When};
use postrinch_store::Store;

use crate::Commands;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

fn message(me: &EmailAddress, id: &str, at: i64, from: &str, labels: &[&str]) -> Message {
    Message {
        account: me.clone(),
        id: ProviderMessageId(id.into()),
        thread: ProviderThreadId(format!("t-{id}")),
        headers: MessageHeaders {
            subject: Some("Field trip forms".into()),
            from: vec![Mailbox::new(None, address(from))],
            to: vec![Mailbox::new(None, me.clone())],
            ..Default::default()
        },
        received: Timestamp::from_millis(at),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        flags: Flags {
            unread: true,
            starred: false,
        },
        snippet: String::new(),
        body_text: "please sign".into(),
        downloaded: true,
        attachments: Vec::new(),
        size: 0,
    }
}

fn setup() -> (tempfile::TempDir, Store, Commands, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = address("me@example.org");
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    for (id, name, kind) in [
        ("INBOX", "INBOX", LabelKind::System(SystemLabel::Inbox)),
        ("SENT", "SENT", LabelKind::System(SystemLabel::Sent)),
        ("Label_1", "School", LabelKind::User),
        ("Label_9", "[Postrinch]/r1", LabelKind::User),
    ] {
        store.put_label(&me, &Label::new(id, name, kind)).unwrap();
    }
    let commands = Commands::new(store.clone());
    (dir, store, commands, me)
}

fn school(id: u32) -> Rule {
    Rule {
        conditions: vec![Condition::is(Test::FromDomain("westfield.edu".into()))],
        actions: vec![Action::AddLabel("Label_1".into()), Action::SkipInbox],
        ..Rule::new(id, "School")
    }
}

fn labels(store: &Store, me: &EmailAddress, id: &str) -> Vec<String> {
    store
        .message(me, &ProviderMessageId(id.into()))
        .unwrap()
        .unwrap()
        .labels
}

#[test]
fn the_first_run_only_notes_where_to_start() {
    let (_dir, store, commands, me) = setup();
    store
        .put_message(
            &message(&me, "old", 100, "a@westfield.edu", &["INBOX"]),
            None,
        )
        .unwrap();
    commands.save_rule(&me, school(0)).unwrap();
    let ran = commands
        .run_rules(&me, Timestamp::from_millis(1_000))
        .unwrap();
    assert_eq!(ran.changed, 0);
    assert_eq!(labels(&store, &me, "old"), ["INBOX"]);
}

#[test]
fn new_mail_goes_through_local_rules_in_order() {
    let (_dir, store, commands, me) = setup();
    commands.save_rule(&me, school(0)).unwrap();
    let stop = Rule {
        stop: true,
        conditions: vec![Condition::is(Test::FromAddress(
            "head@westfield.edu".into(),
        ))],
        actions: vec![Action::Star],
        ..Rule::new(0, "Head")
    };
    let head = commands.save_rule(&me, stop).unwrap();
    commands.move_rule(&me, head, 0).unwrap();
    commands
        .run_rules(&me, Timestamp::from_millis(1_000))
        .unwrap();
    for (id, from) in [
        ("m1", "teacher@westfield.edu"),
        ("m2", "head@westfield.edu"),
        ("m3", "a@mail.westfield.edu"),
        ("m4", "me@example.org"),
    ] {
        let labels: &[&str] = if id == "m4" { &["SENT"] } else { &["INBOX"] };
        store
            .put_message(&message(&me, id, 2_000, from, labels), None)
            .unwrap();
    }
    let ran = commands
        .run_rules(&me, Timestamp::from_millis(3_000))
        .unwrap();
    assert_eq!(labels(&store, &me, "m1"), ["Label_1"]);
    let m2 = store
        .message(&me, &ProviderMessageId("m2".into()))
        .unwrap()
        .unwrap();
    assert!(m2.flags.starred);
    assert_eq!(m2.labels, ["INBOX"], "stopped before School");
    assert_eq!(
        labels(&store, &me, "m3"),
        ["INBOX"],
        "a subdomain is not the domain"
    );
    assert_eq!(
        labels(&store, &me, "m4"),
        ["SENT"],
        "mail sent is left alone"
    );
    assert_eq!(ran.changed, 2);
    // Run again: nothing new.
    assert_eq!(
        commands
            .run_rules(&me, Timestamp::from_millis(4_000))
            .unwrap()
            .changed,
        0
    );
}

#[test]
fn a_body_test_waits_for_the_download() {
    let (_dir, store, commands, me) = setup();
    let rule = Rule {
        conditions: vec![Condition::is(Test::Words("sign".into()))],
        actions: vec![Action::Star],
        ..Rule::new(0, "Sign")
    };
    commands.save_rule(&me, rule).unwrap();
    commands
        .run_rules(&me, Timestamp::from_millis(1_000))
        .unwrap();
    let mut m = message(&me, "m1", 2_000, "a@example.com", &["INBOX"]);
    m.downloaded = false;
    m.body_text.clear();
    store.put_message(&m, None).unwrap();
    assert_eq!(
        commands
            .run_rules(&me, Timestamp::from_millis(3_000))
            .unwrap()
            .changed,
        0
    );
    let (_, local) = store.rules(&me).unwrap();
    assert_eq!(local.pending, ["m1"]);
    store
        .put_message(
            &message(&me, "m1", 2_000, "a@example.com", &["INBOX"]),
            None,
        )
        .unwrap();
    assert_eq!(
        commands
            .run_rules(&me, Timestamp::from_millis(4_000))
            .unwrap()
            .changed,
        1
    );
    assert!(store.rules(&me).unwrap().1.pending.is_empty());
}

#[test]
fn a_split_rule_finishes_what_gmail_marked() {
    let (_dir, store, commands, me) = setup();
    let rule = Rule {
        conditions: vec![Condition::is(Test::FromAddress("a@example.com".into()))],
        actions: vec![
            Action::Star,
            Action::AskAgent {
                agent: "agent-1".into(),
                prompt: "Summarise".into(),
            },
        ],
        ..Rule::new(1, "Summaries")
    };
    store
        .update_rules(&me, |s, l| {
            s.rules.put(rule);
            l.server = Some(true);
            l.cursor = Some(Timestamp::from_millis(1_000));
            let filter = Filter {
                rule: 1,
                query: "from:(a@example.com)".into(),
                add: vec!["STARRED".into()],
                remove: Vec::new(),
                forward: None,
                marker: true,
            };
            s.made.push(Made {
                id: "f1".into(),
                filter,
                since: Timestamp::from_millis(500),
            });
        })
        .unwrap();
    let mut m = message(&me, "m1", 2_000, "a@example.com", &["INBOX", "Label_9"]);
    m.flags.starred = true;
    store.put_message(&m, None).unwrap();
    let ran = commands
        .run_rules(&me, Timestamp::from_millis(3_000))
        .unwrap();
    assert_eq!(ran.asks.len(), 1, "asked once, by the marker");
    assert_eq!(ran.asks[0].prompt, "Summarise");
    assert_eq!(labels(&store, &me, "m1"), ["INBOX"], "marker removed");
    assert!(
        commands
            .run_rules(&me, Timestamp::from_millis(4_000))
            .unwrap()
            .asks
            .is_empty()
    );
}

#[test]
fn waiting_rules_look_at_the_inbox_hourly() {
    let (_dir, store, commands, me) = setup();
    let day = 24 * 60 * 60 * 1000;
    let rule = Rule {
        when: When::Waited,
        conditions: vec![Condition::is(Test::OlderThanDays(7))],
        actions: vec![Action::SkipInbox],
        ..Rule::new(0, "Old")
    };
    commands.save_rule(&me, rule).unwrap();
    store
        .put_message(&message(&me, "old", 0, "a@example.com", &["INBOX"]), None)
        .unwrap();
    store
        .put_message(
            &message(&me, "new", 6 * day, "a@example.com", &["INBOX"]),
            None,
        )
        .unwrap();
    commands
        .run_rules(&me, Timestamp::from_millis(8 * day))
        .unwrap();
    commands
        .run_rules(&me, Timestamp::from_millis(8 * day + 1))
        .unwrap();
    assert!(labels(&store, &me, "old").is_empty());
    assert_eq!(labels(&store, &me, "new"), ["INBOX"]);
}

#[test]
fn already_here_and_applying_to_it() {
    let (_dir, store, commands, me) = setup();
    for (id, from) in [
        ("m1", "a@westfield.edu"),
        ("m2", "b@westfield.edu"),
        ("m3", "c@example.com"),
    ] {
        store
            .put_message(&message(&me, id, 1_000, from, &["INBOX"]), None)
            .unwrap();
    }
    let now = Timestamp::from_millis(2_000);
    let here = commands.already_here(&me, &school(1), now).unwrap();
    assert_eq!((here.threads.len(), here.messages), (2, 2));
    assert_eq!(commands.apply_to_existing(&me, &school(1), now).unwrap(), 2);
    assert_eq!(labels(&store, &me, "m1"), ["Label_1"]);
    assert_eq!(labels(&store, &me, "m3"), ["INBOX"]);
}

#[test]
fn saving_checks_and_marks_the_rules_changed() {
    let (_dir, store, commands, me) = setup();
    assert!(commands.save_rule(&me, Rule::new(0, "Empty")).is_err());
    let id = commands.save_rule(&me, school(0)).unwrap();
    assert_eq!(id, 1);
    assert!(store.rules(&me).unwrap().1.dirty);
    commands.set_rule_on(&me, id, false).unwrap();
    assert!(!commands.rules_page(&me).unwrap().rules[0].enabled);
    commands.delete_rule(&me, id).unwrap();
    assert!(commands.rules_page(&me).unwrap().rules.is_empty());
}
