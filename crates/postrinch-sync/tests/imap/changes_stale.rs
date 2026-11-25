//! Copies another client expunged since sync read them. A change that
//! deletes a message's other copies never moves or copies from one of
//! those (the command would do nothing, and the deletes would take the
//! last real copy), and deletes only once the server says the copy it
//! counts on was made. A label added from one that is gone is copied from
//! one that is there, and a copy not found is looked for again, never made
//! twice.

use chrono::DateTime;
use postrinch_mailtest::{Extension, ImapFault};
use postrinch_model::MessageChange;
use postrinch_sync::SyncError;

use super::changes_support::{all_have, change, count, expunge_copy, put, record, sync_agrees};
use super::sync_support::{Fixture, fixture, fixture_without, mail};

/// Standard folders, and Work and Home.
fn folders(f: &Fixture) {
    f.user.create_standard_folders().unwrap();
    for folder in ["Work", "Home"] {
        f.user.create_folder(folder).unwrap();
    }
}

/// Moves a@x out of the inbox and Work into Home: the inbox copy moves,
/// and Work's is deleted.
fn refile(f: &Fixture) -> MessageChange {
    MessageChange {
        add_labels: vec![f.label_id("Home")],
        remove_labels: vec!["INBOX".to_owned(), f.label_id("Work")],
        ..MessageChange::default()
    }
}

#[tokio::test]
async fn trashing_from_a_copy_another_client_expunged_keeps_the_real_one() {
    let f = fixture().await;
    folders(&f);
    put(&f, "a@x", &["INBOX", "Work"]);
    put(&f, "b@x", &["INBOX", "Work"]);
    f.sync().await;
    // Whichever copy the change moves to Trash, one message has lost it.
    expunge_copy(&f, "INBOX", "a@x");
    expunge_copy(&f, "Work", "b@x");
    let report = change(&f, &["a@x", "b@x"], MessageChange::add_label("TRASH")).await;
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Work"), count(&f, "Trash")),
        (0, 0, 2),
        "each message moved to Trash from the copy that was there"
    );
    assert_eq!(f.labels("a@x"), ["TRASH"]);
    assert_eq!(f.labels("b@x"), ["TRASH"]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn moving_from_a_copy_another_client_expunged_keeps_the_real_one() {
    let f = fixture().await;
    folders(&f);
    put(&f, "a@x", &["INBOX", "Work"]);
    f.sync().await;
    expunge_copy(&f, "INBOX", "a@x");
    let report = change(&f, &["a@x"], refile(&f)).await;
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!((count(&f, "Work"), count(&f, "Home")), (0, 1));
    assert_eq!(f.labels("a@x"), [f.label_id("Home")]);
    sync_agrees(&f).await;
}

/// The inbox copy is expunged by another client just as the change moves
/// it: after the server was seen to have it, before the `UID MOVE`.
async fn expunged_mid_change(f: &Fixture) {
    folders(f);
    put(f, "a@x", &["INBOX", "Work"]);
    f.sync().await;
    let (user, uid) = (
        f.user.clone(),
        super::changes_support::uid_of(f, "INBOX", "a@x"),
    );
    f.server.on_next("UID MOVE", "", move || {
        user.expunge("INBOX", uid).unwrap();
    });
    record(f, &["a@x"], refile(f));
    let first = f.sync.send_outbox(&f.me).await;
    assert!(matches!(first, Err(SyncError::Server { .. })), "{first:?}");
    assert_eq!(count(f, "Work"), 1, "the only real copy stays");
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 1);
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!((count(f, "Work"), count(f, "Home")), (0, 1));
    sync_agrees(f).await;
}

#[tokio::test]
async fn a_move_the_server_did_not_confirm_deletes_nothing() {
    expunged_mid_change(&fixture().await).await;
}

#[tokio::test]
async fn a_move_not_found_by_message_id_deletes_nothing() {
    expunged_mid_change(&fixture_without(&[Extension::Uidplus]).await).await;
}

/// a@x in the inbox and, earlier, in Work, so a copy of it is made from
/// Work's; another client has expunged that one since sync read it.
async fn source_gone() -> Fixture {
    let f = fixture_without(&[Extension::Uidplus]).await;
    folders(&f);
    let earlier = DateTime::parse_from_rfc3339("2026-01-01T09:00:00+00:00").unwrap();
    f.user
        .append("Work", mail(Some("a@x"), "a@x", "").internal_date(earlier))
        .unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    expunge_copy(&f, "Work", "a@x");
    f
}

#[tokio::test]
async fn a_label_added_from_a_copy_that_is_gone_is_copied_from_one_there() {
    let f = source_gone().await;
    record(&f, &["a@x"], MessageChange::add_label(f.label_id("Home")));
    record(&f, &["a@x"], MessageChange::starred(true));
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (2, 0));
    assert_eq!(count(&f, "Home"), 1, "copied from the inbox's copy");
    assert!(all_have(&f, "Home", "\\Flagged"));
    assert!(all_have(&f, "INBOX", "\\Flagged"));
    let mut labels = vec!["INBOX".to_owned(), f.label_id("Home")];
    labels.sort();
    assert_eq!(f.labels("a@x"), labels, "the label stays");
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_copy_not_found_is_looked_for_again_on_the_next_send() {
    let f = source_gone().await;
    // The server would not list Home's new mail, so whether the copy was
    // made is not known: the change is tried again.
    f.server
        .fail_next_matching("UID FETCH", ":*", 1, ImapFault::No("Not now".into()));
    record(&f, &["a@x"], MessageChange::add_label(f.label_id("Home")));
    record(&f, &["a@x"], MessageChange::starred(true));
    let first = f.sync.send_outbox(&f.me).await;
    assert!(matches!(first, Err(SyncError::Server { .. })), "{first:?}");
    assert_eq!(count(&f, "Home"), 1);
    // Sent again, the copy is found in Home, not made twice.
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (2, 0));
    assert_eq!(count(&f, "Home"), 1, "not copied twice");
    assert!(all_have(&f, "Home", "\\Flagged"));
    assert!(all_have(&f, "INBOX", "\\Flagged"));
    sync_agrees(&f).await;
}
