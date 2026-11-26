//! Changes the server will not take: refused for good (dropped, and the
//! message shown as the server has it), busy (tried again), and declined
//! in words that may pass (tried again, then given up on).

use postrinch_mailtest::ImapFault;
use postrinch_model::MessageChange;
use postrinch_sync::SyncError;

use super::changes_support::{change, count, put, record, sync_agrees};
use super::sync_support::fixture;

fn archive() -> MessageChange {
    MessageChange::remove_label("INBOX")
}

#[tokio::test]
async fn a_change_refused_for_good_is_dropped_and_undone_here() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    f.server.fail_next(
        "UID MOVE",
        1,
        ImapFault::No("[NOPERM] You may not file mail there".into()),
    );
    let report = change(&f, &["a@x"], archive()).await;
    assert_eq!(report.sent, 0);
    assert_eq!(report.refused.len(), 1);
    assert!(
        report.refused[0].contains("You may not file mail there"),
        "{:?}",
        report.refused
    );
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 0);
    assert_eq!(f.labels("a@x"), ["INBOX"], "shown as the server has it");
    assert_eq!(count(&f, "INBOX"), 1);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_busy_folder_is_tried_again() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    f.server.fail_next(
        "UID MOVE",
        1,
        ImapFault::No("[INUSE] Mailbox is locked".into()),
    );
    record(&f, &["a@x"], archive());
    let busy = f.sync.send_outbox(&f.me).await;
    assert!(matches!(busy, Err(SyncError::Busy { .. })), "{busy:?}");
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 1);
    assert!(f.labels("a@x").is_empty(), "still shown out of the inbox");
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(count(&f, "Archive"), 1);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_change_declined_again_and_again_is_given_up_on() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    f.server
        .fail_next("UID STORE", 5, ImapFault::No("Try again".into()));
    record(&f, &["a@x"], MessageChange::read());
    record(&f, &["b@x"], archive());
    for _ in 0..4 {
        let declined = f.sync.send_outbox(&f.me).await;
        assert!(
            matches!(declined, Err(SyncError::Server { .. })),
            "{declined:?}"
        );
        // The archive waits behind it, in order.
        assert_eq!(count(&f, "INBOX"), 2);
    }
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 1));
    assert!(
        report.refused[0].contains("Try again"),
        "{:?}",
        report.refused
    );
    assert!(
        f.message("a@x").unwrap().flags.unread,
        "shown unread, as it is"
    );
    assert_eq!(count(&f, "Archive"), 1);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn one_refused_in_a_run_does_not_drop_the_others() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    // Two archives in a row go as one MOVE; refused, each goes alone.
    f.server.fail_next(
        "UID MOVE",
        1,
        ImapFault::No("[CANNOT] Not now, not together".into()),
    );
    record(&f, &["a@x"], archive());
    record(&f, &["b@x"], archive());
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (2, 0));
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (0, 2));
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_folder_gone_from_the_server_refuses_the_change() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    let work = f.label_id("Work");
    f.user.delete_folder("Work").unwrap();
    let report = change(&f, &["a@x"], MessageChange::add_label(&work)).await;
    assert_eq!((report.sent, report.refused.len()), (0, 1));
    assert_eq!(f.labels("a@x"), ["INBOX"]);
}
