//! Copies asked for and not seen made (docs/imap.md, "Keeping every
//! message safe"): one is never made twice, by any kind of change, and
//! the attempts kept for them do not stay for ever.

use postrinch_mailtest::{Extension, ImapFault};
use postrinch_model::{MessageChange, ProviderMessageId};

use super::changes_support::{all_have, count, put, record};
use super::sync_support::{Fixture, fixture, fixture_without, mail};

/// The one stored message, which has no `Message-ID`, in the inbox.
async fn without_message_id(f: &Fixture) -> ProviderMessageId {
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    f.user.append("INBOX", mail(None, "No id", "")).unwrap();
    f.sync().await;
    let ids = f.store.message_ids(&f.me).unwrap();
    assert_eq!(ids.len(), 1);
    ids[0].clone()
}

/// Makes `change` to `id` and sends it, with the connection lost as the
/// copy is made (`fault`), so the server never says whether it was.
async fn lose_the_copy(
    f: &Fixture,
    id: &ProviderMessageId,
    change: &MessageChange,
    fault: ImapFault,
) {
    f.server.fail_next("UID COPY", 1, fault);
    f.store
        .change_messages(&f.me, std::slice::from_ref(id), change)
        .unwrap();
    assert!(f.sync.send_outbox(&f.me).await.is_err());
}

#[tokio::test]
async fn moves_to_the_bin_or_the_archive_are_never_made_twice() {
    for (change, bin) in [
        (MessageChange::add_label("TRASH"), "Trash"),
        (MessageChange::remove_label("INBOX"), "Archive"),
    ] {
        let f = fixture_without(&[Extension::Move]).await;
        let id = without_message_id(&f).await;
        lose_the_copy(&f, &id, &change, ImapFault::DisconnectAfter).await;
        assert_eq!((count(&f, "INBOX"), count(&f, bin)), (1, 1), "{bin}");
        let report = f.sync.send_outbox(&f.me).await.unwrap();
        assert_eq!((report.sent, report.refused.len()), (0, 1), "{bin}");
        assert_eq!((count(&f, "INBOX"), count(&f, bin)), (1, 1), "{bin}");
    }
}

#[tokio::test]
async fn an_attempt_goes_once_sync_has_read_its_folder_past_it() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    let id = without_message_id(&f).await;
    // Without COPYUID or a Message-ID, the copy cannot be found.
    let work = MessageChange::add_label(f.label_id("Work"));
    f.store
        .change_messages(&f.me, std::slice::from_ref(&id), &work)
        .unwrap();
    f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!(count(&f, "Work"), 1);
    assert_eq!(f.store.imap_copy_attempts(&f.me, &id).unwrap().len(), 1);
    // Sync reads Work past it, and keeps the copy as a message of its own.
    f.sync().await;
    assert!(f.store.imap_copy_attempts(&f.me, &id).unwrap().is_empty());
}

#[tokio::test]
async fn an_attempt_whose_message_is_gone_is_swept() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    let id = without_message_id(&f).await;
    let work = MessageChange::add_label(f.label_id("Work"));
    lose_the_copy(&f, &id, &work, ImapFault::DisconnectBefore).await;
    assert_eq!(f.store.imap_copy_attempts(&f.me, &id).unwrap().len(), 1);
    // Work's UIDs come to name other messages, so sync never reads past
    // the attempt; and another client expunges the message.
    f.user.set_uid_validity("Work", 99).unwrap();
    let uid = f.user.messages("INBOX").unwrap()[0].uid;
    f.user.expunge("INBOX", uid).unwrap();
    f.sync().await;
    assert!(f.store.imap_message_places(&f.me, &id).unwrap().is_empty());
    assert!(f.store.imap_copy_attempts(&f.me, &id).unwrap().is_empty());
}

#[tokio::test]
async fn an_attempt_in_a_folder_gone_from_the_server_holds_nothing_up() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    let id = f.id("a@x");
    let work = MessageChange::add_label(f.label_id("Work"));
    lose_the_copy(&f, &id, &work, ImapFault::DisconnectBefore).await;
    f.user.delete_folder("Work").unwrap();
    record(&f, &["a@x"], MessageChange::starred(true));
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 1));
    assert!(all_have(&f, "INBOX", "\\Flagged"));
    assert!(f.store.imap_copy_attempts(&f.me, &id).unwrap().is_empty());
}
