//! Postrinch's own `\Deleted` marks are noted before they are made, so one
//! stranded before its expunge (a connection lost between the two) is
//! still known as ours and expunged at the next send, never taken for
//! another client's mail marked mid-delete (docs/imap.md, "Keeping every
//! message safe").

use postrinch_mailtest::{Extension, ImapFault};
use postrinch_model::MessageChange;

use super::changes_support::{change, count, put, record, sync_agrees};
use super::sync_support::{fixture, fixture_without};

#[tokio::test]
async fn a_mark_stranded_before_uid_expunge_is_expunged_at_the_next_send() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX", "Work"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    f.server
        .fail_next("UID EXPUNGE", 1, ImapFault::DisconnectBefore);
    record(
        &f,
        &["a@x"],
        MessageChange::remove_label(f.label_id("Work")),
    );
    assert!(f.sync.send_outbox(&f.me).await.is_err());
    assert_eq!(count(&f, "Work"), 1, "marked, not expunged");
    f.sync().await;
    // The change is sent again, with the star.
    let report = change(&f, &["b@x"], MessageChange::starred(true)).await;
    assert_eq!((report.sent, report.refused.len()), (2, 0));
    assert_eq!((count(&f, "INBOX"), count(&f, "Work")), (2, 0));
    sync_agrees(&f).await;
    assert_eq!(f.labels("a@x"), ["INBOX"]);
}

#[tokio::test]
async fn without_uidplus_a_mark_stranded_before_expunge_never_holds_later_changes() {
    let f = fixture_without(&[Extension::Move, Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    f.server
        .fail_next("EXPUNGE", 1, ImapFault::DisconnectBefore);
    record(&f, &["a@x"], MessageChange::remove_label("INBOX"));
    assert!(f.sync.send_outbox(&f.me).await.is_err());
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (2, 1));
    // Sync sees a@x's inbox copy marked: it is still known as ours, so it
    // goes with the next expunge rather than being left for another
    // client.
    f.sync().await;
    let report = change(&f, &["b@x"], MessageChange::remove_label("INBOX")).await;
    assert_eq!((report.sent, report.refused.len()), (2, 0));
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (0, 2));
    sync_agrees(&f).await;
}
