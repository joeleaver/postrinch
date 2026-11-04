//! Starting drafts from mail, and what becomes of them.

use postrinch_commands::{CommandError, Commands, Start};
use postrinch_model::{
    Account, Draft, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders,
    Proposal, Provider, ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};
use postrinch_store::{OutgoingKind, Store};

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

fn mailbox(name: &str, a: &str) -> Mailbox {
    Mailbox::new(Some(name.into()), address(a))
}

/// Ada wrote to me, copying Bob.
fn setup() -> (tempfile::TempDir, Store, Commands, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = address("me@example.org");
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    let inbox = Label::new("INBOX", "INBOX", LabelKind::System(SystemLabel::Inbox));
    store.put_label(&me, &inbox).unwrap();
    let message = Message {
        account: me.clone(),
        id: ProviderMessageId("m1".into()),
        thread: ProviderThreadId("t1".into()),
        headers: MessageHeaders {
            message_id: Some("m1@example.org".into()),
            references: vec!["m0@example.org".into()],
            subject: Some("Thursday rota".into()),
            from: vec![mailbox("Ada", "ada@example.org")],
            to: vec![mailbox("Me", "me@example.org")],
            cc: vec![mailbox("Bob", "bob@example.org")],
            ..Default::default()
        },
        received: Timestamp::from_millis(1_762_200_000_000),
        labels: vec!["INBOX".into()],
        flags: Flags::default(),
        snippet: String::new(),
        body_text: "Who can cover Thursday?\n\nAda".into(),
        downloaded: true,
        attachments: Vec::new(),
        size: 0,
    };
    store.put_message(&message, None).unwrap();
    let commands = Commands::new(store.clone());
    (dir, store, commands, me)
}

fn answering<'a>(
    t: &'a ProviderThreadId,
    m: &'a ProviderMessageId,
) -> Option<(&'a ProviderThreadId, &'a ProviderMessageId)> {
    Some((t, m))
}

#[test]
fn replies_go_to_the_right_people_in_the_right_thread() {
    let (_dir, _store, commands, me) = setup();
    let (t, m) = (
        ProviderThreadId("t1".into()),
        ProviderMessageId("m1".into()),
    );

    let reply = commands
        .start_draft(&me, answering(&t, &m), &Start::Reply)
        .unwrap();
    assert_eq!(reply.to, [mailbox("Ada", "ada@example.org")]);
    assert!(reply.cc.is_empty());
    assert_eq!(reply.subject, "Re: Thursday rota");
    assert!(
        reply
            .body
            .contains("Ada wrote:\n> Who can cover Thursday?\n>\n> Ada"),
        "{}",
        reply.body
    );
    assert_eq!(reply.answering.thread, Some(t.clone()));
    assert_eq!(
        reply.answering.in_reply_to.as_deref(),
        Some("m1@example.org")
    );
    assert_eq!(
        reply.answering.references,
        ["m0@example.org", "m1@example.org"]
    );

    let all = commands
        .start_draft(&me, answering(&t, &m), &Start::ReplyAll)
        .unwrap();
    assert_eq!(
        all.cc,
        [mailbox("Bob", "bob@example.org")],
        "everyone but me"
    );

    let forward = commands
        .start_draft(&me, answering(&t, &m), &Start::Forward)
        .unwrap();
    assert!(forward.to.is_empty());
    assert_eq!(forward.subject, "Fwd: Thursday rota");
    assert!(
        forward
            .body
            .contains("---------- Forwarded message ----------\nFrom: Ada <ada@example.org>")
    );

    let fresh = commands.start_draft(&me, None, &Start::New).unwrap();
    assert_eq!(fresh, Draft::new(me.clone()));
}

#[test]
fn sending_needs_someone_and_settles_a_proposal() {
    let (_dir, store, commands, me) = setup();
    let mut draft = Draft::new(me.clone());
    draft.subject = "Hello".into();
    assert!(matches!(
        commands.send_draft(&draft, None),
        Err(CommandError::Invalid(_))
    ));
    assert!(
        matches!(
            commands.propose_draft(&draft),
            Err(CommandError::Invalid(_))
        ),
        "a proposal names its agent"
    );

    draft.to = vec![mailbox("Ada", "ada@example.org")];
    draft.proposal = Some(Proposal {
        agent: "Claude Code".into(),
        reason: Some("You asked me to".into()),
    });
    let proposal = commands.propose_draft(&draft).unwrap();
    let waiting = commands.outgoing().unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].kind, OutgoingKind::Proposed);

    commands.send_draft(&draft, Some(proposal)).unwrap();
    let kinds: Vec<OutgoingKind> = store.outgoing().unwrap().iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [OutgoingKind::Send],
        "sent, and the proposal settled"
    );

    let elsewhere = commands.limited_to(&[address("other@example.org")]);
    assert!(elsewhere.outgoing().unwrap().is_empty());
    assert!(elsewhere.send_draft(&draft, None).is_err());
}
