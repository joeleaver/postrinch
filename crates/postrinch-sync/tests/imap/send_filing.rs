//! Filing sent mail in Sent: learning whether the provider files it
//! itself, keeping settings saved meanwhile, and what a delivered message
//! does when Sent refuses its copy or an attachment has gone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use postrinch_mailtest::{ImapFault, NewMessage};
use postrinch_model::DraftAttachment;
use postrinch_store::{OutgoingKind, SendStage, SentBackOff};
use postrinch_sync::SyncError;
use postrinch_sync::imap::message_id;

use crate::send_support::{SendFixture, header_of, send_fixture};

/// Sends one message, which is delivered.
async fn send(f: &SendFixture, subject: &str) {
    f.queue(&f.draft(subject), OutgoingKind::Send);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1, "{subject}");
}

#[tokio::test]
async fn a_provider_said_to_file_sent_mail_is_given_up_on_only_after_misses_in_a_row() {
    let f = send_fixture(true, true).await;
    // A provider slow to file: each look misses, and gets a copy.
    send(&f, "One").await;
    send(&f, "Two").await;
    assert!(f.settings().server_files_sent, "two misses are not enough");
    assert_eq!(f.folder("Sent").len(), 2);
    send(&f, "Three").await;
    assert!(!f.settings().server_files_sent);
    assert_eq!(f.folder("Sent").len(), 3);

    // From now on it is filed without looking first.
    let searches = f.imap_commands("UID SEARCH");
    send(&f, "Four").await;
    assert_eq!(f.folder("Sent").len(), 4);
    assert_eq!(f.imap_commands("UID SEARCH"), searches);
}

/// Plays a provider that files the last message sent in Sent, but only
/// now, after the send's look gave up; sync then sees it.
async fn files_late(f: &SendFixture) {
    let sent = f.smtp.messages().pop().expect("a message sent");
    f.user
        .append("Sent", NewMessage::new(sent.data).flags(&["\\Seen"]))
        .unwrap();
    f.sync().await;
}

#[tokio::test]
async fn a_provider_that_files_sent_mail_only_after_the_look_is_given_up_on() {
    let f = send_fixture(true, true).await;
    for subject in ["One", "Two"] {
        send(&f, subject).await;
        files_late(&f).await;
        assert!(f.settings().server_files_sent, "{subject}");
    }
    // The copies it filed late were Postrinch's own after a miss: they
    // are not copies found in time, and the misses go on.
    send(&f, "Three").await;
    assert!(!f.settings().server_files_sent);
    files_late(&f).await;

    // Off, and within the back-off (an hour): its second copies leave it
    // off.
    for subject in ["Four", "Five"] {
        let searches = f.imap_commands("UID SEARCH");
        send(&f, subject).await;
        assert_eq!(
            f.imap_commands("UID SEARCH"),
            searches,
            "{subject}: no wait"
        );
        files_late(&f).await;
        assert!(!f.settings().server_files_sent, "{subject}");
    }
}

#[tokio::test]
async fn a_provider_late_only_during_an_outage_gets_its_setting_back() {
    let back_off = SentBackOff {
        first: Duration::from_millis(300),
        longest: Duration::from_secs(1),
    };
    let f = send_fixture(true, true).await.backing_off(back_off);
    // An outage: three sends in a row are filed only after the look.
    for subject in ["One", "Two", "Three"] {
        send(&f, subject).await;
        files_late(&f).await;
    }
    assert!(!f.settings().server_files_sent);

    // It files promptly again; once the back-off has passed, its second
    // copy of the last one gives it its setting back, and the duplicates
    // stop.
    f.files_sent_mail();
    tokio::time::sleep(back_off.first).await;
    let appends = f.imap_commands("APPEND");
    for subject in ["Four", "Five", "Six"] {
        send(&f, subject).await;
        assert!(f.settings().server_files_sent, "{subject}");
    }
    assert_eq!(f.imap_commands("APPEND"), appends, "no second copies");
    assert_eq!(f.folder("Sent").len(), 2 * 3 + 3);
}

#[tokio::test]
async fn a_copy_the_provider_filed_starts_the_misses_again() {
    let f = send_fixture(true, true).await;
    let files = Arc::new(AtomicBool::new(false));
    f.files_sent_mail_while(files.clone());
    send(&f, "One").await;
    send(&f, "Two").await;
    files.store(true, Ordering::SeqCst);
    send(&f, "Three").await;
    files.store(false, Ordering::SeqCst);
    send(&f, "Four").await;
    send(&f, "Five").await;
    assert!(f.settings().server_files_sent, "the misses started again");
    send(&f, "Six").await;
    assert!(!f.settings().server_files_sent);
}

#[tokio::test]
async fn a_provider_that_files_sent_mail_after_all_gets_its_setting_back() {
    let f = send_fixture(true, false).await;
    f.files_sent_mail();
    send(&f, "One").await;
    // The provider's copy and Postrinch's.
    assert_eq!(f.folder("Sent").len(), 2);
    f.sync().await;
    let appends = f.imap_commands("APPEND");
    send(&f, "Two").await;
    assert!(f.settings().server_files_sent);
    assert_eq!(
        f.imap_commands("APPEND"),
        appends,
        "not given a second copy"
    );
    assert_eq!(f.folder("Sent").len(), 3);
}

#[tokio::test]
async fn settings_saved_while_a_send_looks_in_sent_are_kept() {
    let f = send_fixture(true, true).await;
    // The person changes a setting while each message goes.
    let (store, me) = (f.store.clone(), f.me.clone());
    let saved = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let count = saved.clone();
    f.smtp.on_accept(move |_| {
        let n = count.fetch_add(1, Ordering::SeqCst) + 1;
        let mut settings = store.account(&me).unwrap().unwrap().imap.unwrap();
        settings.roles.archive = Some(format!("Archive {n}"));
        store.set_imap_settings(&me, &settings).unwrap();
    });
    for (n, subject) in ["One", "Two", "Three"].into_iter().enumerate() {
        send(&f, subject).await;
        let archive = f.settings().roles.archive;
        assert_eq!(archive, Some(format!("Archive {}", n + 1)), "{subject}");
    }
    assert!(!f.settings().server_files_sent);
}

#[tokio::test]
async fn a_copy_sent_refuses_still_takes_the_draft_away() {
    let f = send_fixture(true, false).await;
    let mut draft = f.draft("Thursday");
    f.queue(&draft, OutgoingKind::Keep);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    let kept = header_of(&f.folder("Drafts")[0]);
    f.sync().await;

    draft.replaces = Some(message_id(&f.me, &kept));
    let id = f.queue(&draft, OutgoingKind::Send);
    let full = ImapFault::No("[OVERQUOTA] Mailbox is full".into());
    f.imap.fail_next_matching("APPEND", "Sent", 1, full);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = f.waiting();
    assert_eq!(
        (waiting.stage, waiting.refused),
        (SendStage::Delivered, true)
    );
    let error = waiting.error.unwrap();
    assert!(error.contains("would not keep a copy"), "{error}");
    assert_eq!(f.smtp.messages().len(), 1);
    assert!(
        f.folder("Drafts").is_empty(),
        "the message went: so does the draft"
    );

    // Tried again, it is only filed.
    f.store.outgoing_send_again(id).unwrap();
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1);
    assert_eq!(f.folder("Sent").len(), 1);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_sent_folder_deleted_on_the_server_does_not_stop_the_outbox() {
    for files in [false, true] {
        let f = send_fixture(true, files).await;
        // Deleted there since sync last saw it.
        f.user.delete_folder("Sent").unwrap();
        f.queue(&f.draft("One"), OutgoingKind::Send);
        f.queue(&f.draft("Two"), OutgoingKind::Send);
        assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0, "{files}");
        assert_eq!(f.smtp.messages().len(), 2, "{files}: both went");
        let waiting = f.outgoing();
        assert_eq!(waiting.len(), 2, "{files}");
        for message in waiting {
            assert_eq!(
                (message.stage, message.refused),
                (SendStage::Delivered, true),
                "{files}"
            );
            let error = message.error.unwrap();
            assert!(error.contains("would not keep a copy"), "{files}: {error}");
        }
    }
}

#[tokio::test]
async fn a_busy_answer_to_opening_sent_is_the_server_busy() {
    let f = send_fixture(true, false).await;
    f.queue(&f.draft("One"), OutgoingKind::Send);
    let busy = ImapFault::No("[INUSE] Mailbox is locked".into());
    f.imap.fail_next_matching("EXAMINE", "Sent", 1, busy);
    let failed = f.sync.send_outgoing(&f.me).await;
    assert!(matches!(failed, Err(SyncError::Busy { .. })), "{failed:?}");
    // Delivered, and only filed next time.
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1);
    assert_eq!(f.folder("Sent").len(), 1);
}

#[tokio::test]
async fn a_delivered_message_whose_attachment_went_is_filed_without_it() {
    let f = send_fixture(true, false).await;
    let mut draft = f.draft("Thursday");
    draft.attachments = vec![DraftAttachment {
        name: "menu.pdf".into(),
        mime_type: "application/pdf".into(),
        size: 4,
        blob: "0".repeat(64),
        origin: Default::default(),
    }];
    let id = f.queue(&draft, OutgoingKind::Send);
    f.store.outgoing_stage(id, SendStage::Delivered).unwrap();
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.command_count("MAIL"), 0, "never sent again");
    let sent = f.folder("Sent");
    assert_eq!(sent.len(), 1);
    assert!(!String::from_utf8_lossy(&sent[0].body).contains("menu.pdf"));
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn a_message_whose_attachment_went_is_not_sent() {
    let f = send_fixture(true, false).await;
    let mut draft = f.draft("Thursday");
    draft.attachments = vec![DraftAttachment {
        name: "menu.pdf".into(),
        mime_type: "application/pdf".into(),
        size: 4,
        blob: "0".repeat(64),
        origin: Default::default(),
    }];
    f.queue(&draft, OutgoingKind::Send);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let waiting = f.waiting();
    assert!(waiting.refused);
    let error = waiting.error.unwrap();
    assert!(
        error.contains("menu.pdf is no longer on this computer"),
        "{error}"
    );
    assert_eq!(f.smtp.command_count("MAIL"), 0);
}
