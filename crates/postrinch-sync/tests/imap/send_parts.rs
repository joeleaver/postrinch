//! Sending to a server that takes only so many recipients in one
//! transaction (452 at RCPT): the message goes in parts, to each recipient
//! once, and a part that went is never sent again.

use postrinch_mailtest::{SmtpConfig, SmtpFault, SmtpStage};
use postrinch_model::EmailAddress;
use postrinch_store::{OutgoingKind, SendStage};
use postrinch_sync::SyncError;

use crate::send_support::{BCC, CC, SendFixture, TO, reply, send_fixture_with};

async fn taking(most: usize, server_files_sent: bool) -> SendFixture {
    let smtp = SmtpConfig::new().max_recipients(most);
    send_fixture_with(true, server_files_sent, smtp).await
}

/// Whom each message the server took went to, in order.
fn parts(f: &SendFixture) -> Vec<Vec<String>> {
    f.smtp.messages().into_iter().map(|m| m.rcpt_to).collect()
}

fn addresses(list: &[&str]) -> Vec<EmailAddress> {
    list.iter()
        .map(|a| EmailAddress::parse(a).unwrap())
        .collect()
}

#[tokio::test]
async fn a_server_that_takes_few_recipients_at_once_gets_the_message_in_parts() {
    let f = taking(2, false).await;
    f.queue(&f.draft_for_three("Thursday"), OutgoingKind::Send);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(parts(&f), [vec![TO, CC], vec![BCC]]);
    assert_eq!(
        f.smtp.command_count("MAIL"),
        2,
        "one connection, two transactions"
    );
    assert_eq!(f.folder("Sent").len(), 1, "filed once");
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_part_that_fails_is_tried_again_for_the_rest_only() {
    let f = taking(1, false).await;
    f.queue(&f.draft_for_three("Thursday"), OutgoingKind::Send);
    f.smtp
        .fail_after(SmtpStage::Mail, 1, 1, reply(451, "4.3.0 Try again later"));
    let failed = f.sync.send_outgoing(&f.me).await;
    assert!(matches!(failed, Err(SyncError::Busy { .. })), "{failed:?}");
    let waiting = f.waiting();
    assert_eq!(
        (waiting.stage, waiting.refused, waiting.delivered_to.clone()),
        (SendStage::Queued, false, addresses(&[TO]))
    );

    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(parts(&f), [vec![TO], vec![CC], vec![BCC]], "each once");
    assert_eq!(f.folder("Sent").len(), 1);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_part_lost_after_its_data_waits_though_sent_has_the_message() {
    let f = taking(1, true).await;
    f.files_sent_mail();
    let id = f.queue(&f.draft_for_three("Thursday"), OutgoingKind::Send);
    f.smtp
        .fail_after(SmtpStage::Data, 1, 1, SmtpFault::Disconnect);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    // Sent has the message from the first part, which says nothing of
    // the second.
    assert_eq!(f.folder("Sent").len(), 1);
    let waiting = f.waiting();
    assert_eq!(
        (
            waiting.stage,
            waiting.refused,
            waiting.delivered_to.clone(),
            waiting.sending_to.clone()
        ),
        (
            SendStage::DataSent,
            true,
            addresses(&[TO]),
            addresses(&[CC])
        )
    );
    let error = waiting.error.unwrap();
    assert!(
        error.contains("may have gone") && error.contains(TO),
        "{error}"
    );
    assert_eq!(parts(&f), [vec![TO]]);

    // Not sent blind; the person sends it again, to those it may not
    // have reached.
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    assert_eq!(parts(&f), [vec![TO]]);
    f.store.outgoing_send_again(id).unwrap();
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(parts(&f), [vec![TO], vec![CC], vec![BCC]]);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_recipient_refused_in_a_later_part_says_whom_it_went_to() {
    let f = taking(1, false).await;
    f.queue(&f.draft_for_three("Thursday"), OutgoingKind::Send);
    // The first part's RCPTs (the second answered "too many"), then the
    // second part's first.
    f.smtp
        .fail_after(SmtpStage::Rcpt, 2, 1, reply(550, "5.1.1 No such user"));
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = f.waiting();
    assert_eq!(
        (waiting.stage, waiting.refused, waiting.delivered_to.clone()),
        (SendStage::Queued, true, addresses(&[TO]))
    );
    let error = waiting.error.unwrap();
    assert!(
        error.contains(&format!("It went to {TO}")) && error.contains(CC),
        "{error}"
    );
    assert!(error.contains("No such user"), "{error}");
    assert_eq!(parts(&f), [vec![TO]]);
}

#[tokio::test]
async fn too_many_recipients_for_even_the_first_is_the_server_busy() {
    let f = taking(0, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    let failed = f.sync.send_outgoing(&f.me).await;
    assert!(
        matches!(
            failed,
            Err(SyncError::Busy {
                service: "SMTP",
                ..
            })
        ),
        "{failed:?}"
    );
    let waiting = f.waiting();
    assert_eq!((waiting.stage, waiting.refused), (SendStage::Queued, false));
    assert!(f.smtp.messages().is_empty());
}
