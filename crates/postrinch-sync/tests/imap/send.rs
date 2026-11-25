//! Sending an IMAP account's mail over SMTP and filing it in Sent, against
//! the fake servers: each message goes once, whatever the servers do.

use postrinch_mailtest::{SmtpFault, SmtpStage};
use postrinch_store::{OutgoingKind, SendStage};
use postrinch_sync::SyncError;

use crate::send_support::{BCC, SendFixture, TO, header, reply, send_fixture};

/// The header block of a message.
fn head(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    text.split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_message_goes_to_everyone_and_is_filed_in_sent() {
    for uidplus in [true, false] {
        let f = send_fixture(uidplus, false).await;
        f.queue(&f.draft("Thursday"), OutgoingKind::Send);
        assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);

        let sent = f.smtp.messages();
        assert_eq!(sent.len(), 1, "uidplus {uidplus}");
        let mut to = sent[0].rcpt_to.clone();
        to.sort();
        assert_eq!(to, [TO, BCC]);
        assert!(!head(&sent[0].data).contains(BCC), "Bcc is not shown");
        assert!(f.outgoing().is_empty());

        let filed = f.folder("Sent");
        assert_eq!(filed.len(), 1, "uidplus {uidplus}");
        assert!(filed[0].has_flag("\\Seen"));
        assert!(head(&filed[0].body).contains(BCC), "Sent keeps the Bcc");
        assert_eq!(
            header(&filed[0].body, "Message-ID"),
            header(&sent[0].data, "Message-ID")
        );
        let id = f.id_of(&filed[0]);
        // Recorded from APPENDUID, or left to sync.
        assert_eq!(f.places(&id), usize::from(uidplus));
        let before = f.message_count();
        f.sync().await;
        assert_eq!(f.message_count(), before + 1, "stored once");
        assert_eq!(f.places(&id), 1);
    }
}

#[tokio::test]
async fn a_server_that_files_sent_mail_is_not_given_a_second_copy() {
    let f = send_fixture(true, true).await;
    f.files_sent_mail();
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.folder("Sent").len(), 1);
    assert_eq!(f.imap_commands("APPEND"), 0);
    assert!(f.settings().server_files_sent);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_busy_server_before_the_data_is_tried_again() {
    // A 4xx at RCPT is one recipient's own (send_recipients.rs).
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp
        .fail_next(SmtpStage::Mail, 1, reply(451, "4.3.0 Try again later"));
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
    assert_eq!(
        (waiting.attempts, waiting.refused, waiting.stage),
        (1, false, SendStage::Queued)
    );
    assert!(f.smtp.messages().is_empty());
    assert!(f.folder("Sent").is_empty());

    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1);
    assert_eq!(f.folder("Sent").len(), 1);
}

#[tokio::test]
async fn a_refused_message_waits_for_the_person() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp
        .fail_next(SmtpStage::Rcpt, 1, reply(550, "5.1.1 No such user"));
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = f.waiting();
    assert!(waiting.refused);
    let error = waiting.error.unwrap();
    assert!(error.contains("No such user"), "{error}");
    assert_eq!(waiting.stage, SendStage::Queued);
    assert!(f.smtp.messages().is_empty());

    // Not tried again until the person says so.
    let mails = f.smtp.command_count("MAIL");
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    assert_eq!(f.smtp.command_count("MAIL"), mails);
    assert!(f.folder("Sent").is_empty());
}

#[tokio::test]
async fn a_message_lost_after_its_data_and_found_in_sent_was_delivered() {
    let f = send_fixture(true, true).await;
    f.files_sent_mail();
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp
        .fail_next(SmtpStage::Data, 1, SmtpFault::StoreThenDisconnect);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1, "sent once");
    assert_eq!(f.folder("Sent").len(), 1, "filed once");
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_message_lost_after_its_data_and_not_in_sent_waits_and_is_never_sent_blind() {
    let f = send_fixture(true, false).await;
    let id = f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp.fail_next(SmtpStage::Data, 1, SmtpFault::Disconnect);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = f.waiting();
    assert_eq!(
        (waiting.refused, waiting.stage),
        (true, SendStage::DataSent)
    );
    let error = waiting.error.unwrap();
    assert!(error.contains("may have gone"), "{error}");

    let datas = f.smtp.command_count("DATA");
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    assert_eq!(f.smtp.command_count("DATA"), datas, "not sent again");

    // The person sends it again.
    f.store.outgoing_send_again(id).unwrap();
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1);
    assert_eq!(f.folder("Sent").len(), 1);
}

#[tokio::test]
async fn a_message_left_without_an_answer_when_the_app_stopped_is_never_sent_blind() {
    let f = send_fixture(true, true).await;
    let id = f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.store.outgoing_stage(id, SendStage::DataSent).unwrap();
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    assert_eq!(f.smtp.command_count("MAIL"), 0);
    let waiting = f.waiting();
    assert_eq!(
        (waiting.refused, waiting.stage),
        (true, SendStage::DataSent)
    );
}

#[tokio::test]
async fn a_delivered_message_whose_filing_failed_is_only_filed() {
    let f = send_fixture(true, false).await;
    let id = f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.store.outgoing_stage(id, SendStage::Delivered).unwrap();
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.command_count("MAIL"), 0, "never sent again");
    assert_eq!(f.folder("Sent").len(), 1);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_busy_answer_after_the_end_of_the_data_is_tried_again() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp
        .fail_next(SmtpStage::Data, 1, reply(451, "4.3.0 Try again later"));
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
    assert_eq!(
        (waiting.attempts, waiting.refused, waiting.stage),
        (1, false, SendStage::Queued)
    );
    assert!(f.smtp.messages().is_empty());
    assert!(f.folder("Sent").is_empty());

    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1, "delivered once");
    assert_eq!(f.folder("Sent").len(), 1);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_refusal_after_the_end_of_the_data_waits_for_the_person() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    f.smtp.fail_next(
        SmtpStage::Data,
        1,
        reply(554, "5.7.1 Message refused as spam"),
    );
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = f.waiting();
    assert_eq!(
        (waiting.attempts, waiting.refused, waiting.stage),
        (1, true, SendStage::Queued)
    );
    let error = waiting.error.unwrap();
    assert!(error.contains("refused as spam"), "{error}");
    assert!(f.smtp.messages().is_empty());

    let datas = f.smtp.command_count("DATA");
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    assert_eq!(f.smtp.command_count("DATA"), datas, "not tried again");
    assert!(f.folder("Sent").is_empty());
}

#[tokio::test]
async fn two_deliveries_at_once_send_a_message_once() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    let (one, two) = tokio::join!(f.sync.send_outgoing(&f.me), f.sync.send_outgoing(&f.me));
    assert_eq!(one.unwrap() + two.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1, "sent once");
    assert_eq!(f.folder("Sent").len(), 1, "filed once");
    assert!(f.outgoing().is_empty());
}

/// Makes the next `times` writes of outgoing mail fail once the server
/// has taken a message, before it answers.
fn records_fail_after_acceptance(f: &SendFixture, times: usize) {
    let store = f.store.clone();
    f.smtp
        .on_accept(move |_| store.fail_updates("Outgoing", times));
}

#[tokio::test]
async fn a_delivery_recorded_only_when_tried_again_is_done_with() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    records_fail_after_acceptance(&f, 1);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1);
    assert_eq!(f.folder("Sent").len(), 1);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_delivery_that_cannot_be_recorded_waits_for_the_person_with_a_warning() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Send);
    // Every try at recording it fails.
    records_fail_after_acceptance(&f, 3);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = f.waiting();
    assert_eq!(
        (waiting.refused, waiting.stage),
        (true, SendStage::DataSent)
    );
    let error = waiting.error.unwrap();
    assert!(
        error.contains("could not record") && error.contains(TO) && error.contains("second time"),
        "{error}"
    );
    // Never sent again without the person.
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    assert_eq!(f.smtp.messages().len(), 1);
}
