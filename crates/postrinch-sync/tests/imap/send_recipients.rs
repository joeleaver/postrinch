//! How the answers to RCPT are read: a recipient the server puts off for
//! now (a 4xx for them alone, as a full mailbox) does not stop the others,
//! and is tried again later, but not for ever; "too many recipients" is
//! told by its enhanced code, or by its coming after some were taken.

use postrinch_mailtest::SmtpConfig;
use postrinch_model::EmailAddress;
use postrinch_store::{OutgoingKind, SendStage};
use postrinch_sync::SyncError;

use crate::send_support::{BCC, CC, SendFixture, TO, send_fixture, send_fixture_with};

/// Whom each message the server took went to, in order.
fn parts(f: &SendFixture) -> Vec<Vec<String>> {
    f.smtp.messages().into_iter().map(|m| m.rcpt_to).collect()
}

fn addresses(list: &[&str]) -> Vec<EmailAddress> {
    list.iter()
        .map(|a| EmailAddress::parse(a).unwrap())
        .collect()
}

/// A send that put someone off: tried again later, as the server busy,
/// in words that name them.
async fn put_off(f: &SendFixture, whom: &str) {
    let failed = f.sync.send_outgoing(&f.me).await;
    match failed {
        Err(SyncError::Busy {
            service: "SMTP",
            message,
            ..
        }) => assert!(
            message.contains(whom) && message.contains("Mailbox full"),
            "{message}"
        ),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_full_mailbox_puts_off_only_its_recipient() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft_for_three("Thursday"), OutgoingKind::Send);
    f.smtp.answer_recipient(CC, 452, "4.2.2 Mailbox full");
    put_off(&f, CC).await;
    assert_eq!(parts(&f), [vec![TO, BCC]], "the others have it at once");
    let waiting = f.waiting();
    assert_eq!(
        (waiting.stage, waiting.refused, waiting.delivered_to.clone()),
        (SendStage::Queued, false, addresses(&[TO, BCC]))
    );
    assert_eq!(waiting.put_off.len(), 1);
    assert_eq!(waiting.put_off[0].address.to_string(), CC);
    assert!(waiting.error.unwrap().contains(CC));
    assert!(f.folder("Sent").is_empty(), "filed once everyone has it");

    // Room again: it goes to CC only.
    f.smtp.take_recipient(CC);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(parts(&f), [vec![TO, BCC], vec![CC]]);
    assert_eq!(f.folder("Sent").len(), 1);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_message_put_off_for_every_recipient_is_tried_again() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp
        .answer_recipient(TO, 452, "4.3.1 Mailbox full, out of storage");
    f.smtp
        .answer_recipient(BCC, 452, "4.3.1 Mailbox full, out of storage");
    put_off(&f, TO).await;
    assert!(f.smtp.messages().is_empty());
    assert_eq!(f.smtp.command_count("DATA"), 0, "no data for nobody");
    f.smtp.take_recipient(TO);
    f.smtp.take_recipient(BCC);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(parts(&f), [vec![TO, BCC]]);
}

#[tokio::test]
async fn a_recipient_put_off_too_often_waits_for_the_person() {
    let f = send_fixture(true, false).await;
    let id = f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp.answer_recipient(TO, 452, "4.2.2 Mailbox full");
    let mut attempts = 0;
    while f.sync.send_outgoing(&f.me).await.is_err() {
        attempts += 1;
        assert!(attempts < 20, "tried for ever");
    }
    assert!(attempts > 1, "tried again first");
    assert_eq!(parts(&f), [vec![BCC]], "the others got it once");
    let waiting = f.waiting();
    assert!(waiting.refused);
    let error = waiting.error.unwrap();
    assert!(
        error.contains(&format!("It went to {BCC}"))
            && error.contains(TO)
            && error.contains("Mailbox full"),
        "{error}"
    );
    // Left alone until the person sends it again, which tries afresh.
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    f.store.outgoing_send_again(id).unwrap();
    assert!(f.waiting().put_off.is_empty());
    f.smtp.take_recipient(TO);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(parts(&f), [vec![BCC], vec![TO]]);
}

/// A fixture whose server takes one recipient at a time, answering the
/// next with `code` and `text`.
async fn taking_one(code: u16, text: &str) -> SendFixture {
    let smtp = SmtpConfig::new()
        .max_recipients(1)
        .too_many_recipients_answer(code, text);
    send_fixture_with(true, false, smtp).await
}

#[tokio::test]
async fn too_many_recipients_is_known_without_an_enhanced_code_once_some_were_taken() {
    for code in [452, 552] {
        let f = taking_one(code, "Too many recipients").await;
        f.queue(&f.draft_for_three("Thursday"), OutgoingKind::Send);
        assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1, "{code}");
        assert_eq!(parts(&f), [vec![TO], vec![CC], vec![BCC]], "{code}");
        assert!(f.outgoing().is_empty(), "{code}");
    }
}

#[tokio::test]
async fn a_552_that_is_not_too_many_recipients_refuses_the_message() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp
        .answer_recipient(BCC, 552, "5.2.2 Mailbox full for good");
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = f.waiting();
    assert!(waiting.refused);
    let error = waiting.error.unwrap();
    assert!(error.contains(BCC) && error.contains("for good"), "{error}");
    assert!(f.smtp.messages().is_empty());
}
