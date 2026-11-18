use postrinch_model::{
    AttachmentInfo, EmailAddress, Flags, Mailbox, Message, MessageHeaders, ProviderMessageId,
    ProviderThreadId, Timestamp,
};

use super::{Verdict, matches};
use crate::{Condition, Match, Rule, Test};

fn mailbox(name: Option<&str>, address: &str) -> Mailbox {
    Mailbox::new(
        name.map(str::to_owned),
        EmailAddress::parse(address).unwrap(),
    )
}

fn message() -> Message {
    Message {
        account: EmailAddress::parse("me@example.com").unwrap(),
        id: ProviderMessageId("m1".into()),
        thread: ProviderThreadId("t1".into()),
        headers: MessageHeaders {
            subject: Some("Field trip forms due Friday".into()),
            from: vec![mailbox(Some("Ms Okafor"), "okafor@westfield.edu")],
            to: vec![mailbox(None, "me@example.com")],
            cc: vec![mailbox(None, "partner@example.net")],
            ..Default::default()
        },
        received: Timestamp::from_millis(0),
        labels: vec!["INBOX".into()],
        flags: Flags {
            unread: true,
            starred: false,
        },
        snippet: String::new(),
        body_text: "Please sign the permission form for the museum.".into(),
        attachments: vec![AttachmentInfo {
            filename: Some("Permission-Form.PDF".into()),
            mime_type: "application/pdf".into(),
            size: 1000,
            content_id: None,
            inline: false,
        }],
        size: 50_000,
        downloaded: true,
    }
}

fn rule(matching: Match, conditions: Vec<Condition>) -> Rule {
    Rule {
        matching,
        conditions,
        ..Rule::new(1, "R")
    }
}

fn check(test: Test) -> Verdict {
    matches(
        &rule(Match::All, vec![Condition::is(test)]),
        &message(),
        Timestamp::from_millis(0),
    )
}

#[test]
fn domains_are_exact_unlike_gmail() {
    assert_eq!(
        check(Test::FromDomain("westfield.edu".into())),
        Verdict::Yes
    );
    assert_eq!(
        check(Test::FromDomain("@WESTFIELD.edu".into())),
        Verdict::Yes
    );
    assert_eq!(check(Test::FromDomain("edu".into())), Verdict::No);
    let mut m = message();
    m.headers.from = vec![mailbox(None, "a@mail.westfield.edu")];
    let r = rule(
        Match::All,
        vec![Condition::is(Test::FromDomain("westfield.edu".into()))],
    );
    assert_eq!(matches(&r, &m, Timestamp::from_millis(0)), Verdict::No);
}

#[test]
fn addresses_and_recipients() {
    assert_eq!(
        check(Test::FromAddress("Okafor@Westfield.edu".into())),
        Verdict::Yes
    );
    assert_eq!(check(Test::FromWords("okafor".into())), Verdict::Yes);
    assert_eq!(check(Test::FromWords("ms westfield".into())), Verdict::Yes);
    assert_eq!(
        check(Test::RecipientAddress("partner@example.net".into())),
        Verdict::Yes
    );
    assert_eq!(
        check(Test::ToAddress("partner@example.net".into())),
        Verdict::No
    );
    assert_eq!(
        check(Test::CcAddress("partner@example.net".into())),
        Verdict::Yes
    );
}

#[test]
fn words_are_whole() {
    assert_eq!(check(Test::SubjectWords("trip FIELD".into())), Verdict::Yes);
    assert_eq!(check(Test::SubjectWords("trips".into())), Verdict::No);
    assert_eq!(
        check(Test::SubjectPhrase("field trip".into())),
        Verdict::Yes
    );
    assert_eq!(check(Test::SubjectPhrase("trip field".into())), Verdict::No);
    assert_eq!(check(Test::Words("museum okafor".into())), Verdict::Yes);
    assert_eq!(
        check(Test::Words("form pdf".into())),
        Verdict::Yes,
        "attachment names count"
    );
    assert_eq!(check(Test::Phrase("permission form".into())), Verdict::Yes);
}

#[test]
fn attachments_and_size() {
    assert_eq!(check(Test::HasAttachment), Verdict::Yes);
    assert_eq!(check(Test::AttachmentType("pdf".into())), Verdict::Yes);
    assert_eq!(check(Test::AttachmentType(".doc".into())), Verdict::No);
    assert_eq!(
        check(Test::AttachmentName("permission-form.pdf".into())),
        Verdict::Yes
    );
    assert_eq!(check(Test::AttachmentName("form.pdf".into())), Verdict::No);
    assert_eq!(check(Test::LargerThan(10_000)), Verdict::Yes);
    assert_eq!(check(Test::SmallerThan(10_000)), Verdict::No);
}

#[test]
fn state_and_age() {
    assert_eq!(check(Test::Unread), Verdict::Yes);
    assert_eq!(check(Test::Starred), Verdict::No);
    assert_eq!(check(Test::InInbox), Verdict::Yes);
    assert_eq!(check(Test::HasLabel("Label_9".into())), Verdict::No);
    let r = rule(Match::All, vec![Condition::is(Test::OlderThanDays(7))]);
    let day = 24 * 60 * 60 * 1000;
    assert_eq!(
        matches(&r, &message(), Timestamp::from_millis(7 * day)),
        Verdict::No
    );
    assert_eq!(
        matches(&r, &message(), Timestamp::from_millis(8 * day)),
        Verdict::Yes
    );
}

#[test]
fn all_any_and_not() {
    let yes = Condition::is(Test::Unread);
    let no = Condition::is(Test::Starred);
    let now = Timestamp::from_millis(0);
    let m = message();
    assert_eq!(
        matches(&rule(Match::All, vec![yes.clone(), no.clone()]), &m, now),
        Verdict::No
    );
    assert_eq!(
        matches(&rule(Match::Any, vec![yes.clone(), no.clone()]), &m, now),
        Verdict::Yes
    );
    assert_eq!(
        matches(
            &rule(Match::All, vec![yes, Condition::not(Test::Starred)]),
            &m,
            now
        ),
        Verdict::Yes
    );
}

#[test]
fn body_tests_wait_for_the_download() {
    let mut m = message();
    m.downloaded = false;
    let now = Timestamp::from_millis(0);
    let body = Condition::is(Test::Words("museum".into()));
    let from = Condition::is(Test::FromAddress("okafor@westfield.edu".into()));
    let elsewhere = Condition::is(Test::FromAddress("x@example.org".into()));
    assert_eq!(
        matches(&rule(Match::All, vec![from.clone(), body.clone()]), &m, now),
        Verdict::NotYet
    );
    assert_eq!(
        matches(
            &rule(Match::All, vec![elsewhere.clone(), body.clone()]),
            &m,
            now
        ),
        Verdict::No
    );
    assert_eq!(
        matches(&rule(Match::Any, vec![from, body.clone()]), &m, now),
        Verdict::Yes
    );
    assert_eq!(
        matches(&rule(Match::Any, vec![elsewhere, body.clone()]), &m, now),
        Verdict::NotYet
    );
    let not_body = Condition::not(Test::Words("museum".into()));
    assert_eq!(
        matches(&rule(Match::All, vec![not_body]), &m, now),
        Verdict::NotYet
    );
}
