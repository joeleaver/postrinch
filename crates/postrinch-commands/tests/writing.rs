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

/// A newsletter whose text part spells out every link's address, as many
/// senders' do: an answer quotes what its reader sees, the HTML as drawn.
#[test]
fn answers_quote_html_mail_as_drawn_not_its_text_part() {
    let (_dir, store, commands, me) = setup();
    let raw = "From: News <news@example.org>\r\n\
        To: me@example.org\r\n\
        Subject: Today's letter\r\n\
        Message-ID: <n1@example.org>\r\n\
        MIME-Version: 1.0\r\n\
        Content-Type: multipart/alternative; boundary=\"b\"\r\n\
        \r\n\
        --b\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        View this email in your browser (https://example.org/archive?id=123)\r\n\
        A story worth reading (https://example.org/track?u=abc&id=456)\r\n\
        --b\r\n\
        Content-Type: text/html; charset=utf-8\r\n\
        \r\n\
        <p><a href=\"https://example.org/archive?id=123\">View this email in your browser</a></p>\
        <p>A <a href=\"https://example.org/track?u=abc&amp;id=456\">story</a> worth reading</p>\r\n\
        --b--\r\n";
    let message = Message {
        account: me.clone(),
        id: ProviderMessageId("n1".into()),
        thread: ProviderThreadId("tn".into()),
        headers: MessageHeaders {
            message_id: Some("n1@example.org".into()),
            subject: Some("Today's letter".into()),
            from: vec![mailbox("News", "news@example.org")],
            to: vec![mailbox("Me", "me@example.org")],
            ..Default::default()
        },
        received: Timestamp::from_millis(1_762_200_000_000),
        labels: vec!["INBOX".into()],
        flags: Flags::default(),
        snippet: String::new(),
        body_text: "View this email in your browser (https://example.org/archive?id=123)\n\
            A story worth reading (https://example.org/track?u=abc&id=456)"
            .into(),
        downloaded: true,
        attachments: Vec::new(),
        size: raw.len() as u64,
    };
    store.put_message(&message, Some(raw.as_bytes())).unwrap();
    let (t, m) = (
        ProviderThreadId("tn".into()),
        ProviderMessageId("n1".into()),
    );
    for start in [Start::Reply, Start::Forward] {
        let draft = commands
            .start_draft(&me, answering(&t, &m), &start)
            .unwrap();
        assert!(
            draft.body.contains("View this email in your browser"),
            "{}",
            draft.body
        );
        assert!(draft.body.contains("story"), "{}", draft.body);
        assert!(!draft.body.contains("https://"), "{}", draft.body);
    }
}

#[test]
fn a_forward_carries_the_files_and_nothing_goes_over_gmail_s_limit() {
    let (_dir, store, commands, me) = setup();
    let raw = "From: Dana <dana@example.org>\r\n\
        To: me@example.org\r\n\
        Subject: Proofs\r\n\
        Message-ID: <p1@example.org>\r\n\
        MIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\
        \r\n\
        --b\r\n\
        Content-Type: text/plain\r\n\
        \r\n\
        Proofs attached.\r\n\
        --b\r\n\
        Content-Type: text/csv; name=\"prices.csv\"\r\n\
        Content-Disposition: attachment; filename=\"prices.csv\"\r\n\
        Content-Transfer-Encoding: base64\r\n\
        \r\n\
        b25lLHR3bwo=\r\n\
        --b--\r\n";
    let message = Message {
        account: me.clone(),
        id: ProviderMessageId("p1".into()),
        thread: ProviderThreadId("tp".into()),
        headers: MessageHeaders {
            message_id: Some("p1@example.org".into()),
            subject: Some("Proofs".into()),
            from: vec![mailbox("Dana", "dana@example.org")],
            ..Default::default()
        },
        received: Timestamp::from_millis(1_762_200_000_000),
        labels: vec!["INBOX".into()],
        flags: Flags::default(),
        snippet: String::new(),
        body_text: "Proofs attached.".into(),
        downloaded: true,
        attachments: vec![postrinch_model::AttachmentInfo {
            filename: Some("prices.csv".into()),
            mime_type: "text/csv".into(),
            size: 8,
            content_id: None,
            inline: false,
        }],
        size: raw.len() as u64,
    };
    store.put_message(&message, Some(raw.as_bytes())).unwrap();
    let (t, m) = (
        ProviderThreadId("tp".into()),
        ProviderMessageId("p1".into()),
    );
    let reply = commands
        .start_draft(&me, answering(&t, &m), &Start::Reply)
        .unwrap();
    assert!(reply.attachments.is_empty(), "a reply carries no files");

    let mut forward = commands
        .start_draft(&me, answering(&t, &m), &Start::Forward)
        .unwrap();
    assert_eq!(forward.attachments.len(), 1);
    let file = &forward.attachments[0];
    assert_eq!((file.name.as_str(), file.size), ("prices.csv", 8));
    assert_eq!(file.origin, postrinch_model::AttachmentOrigin::Mail);
    assert_eq!(commands.draft_attachment_bytes(file).unwrap(), b"one,two\n");

    forward.to = vec![mailbox("Printer", "printer@example.org")];
    forward.attachments[0].size = postrinch_model::MAX_ATTACHED + 1;
    let refused = commands.send_draft(&forward, None).unwrap_err();
    assert!(matches!(refused, CommandError::Invalid(_)), "{refused}");
    assert!(store.outgoing().unwrap().is_empty());
}
