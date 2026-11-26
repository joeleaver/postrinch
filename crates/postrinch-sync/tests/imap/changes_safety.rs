//! Nothing is deleted or expunged on a guess (docs/imap.md, "Keeping every
//! message safe"): a copy the server did not confirm never keeps a
//! message, is never made twice, and never leaves our marked copies to
//! pile up; a label's mail whose headers never come does not hold its
//! deletion for ever.

use chrono::DateTime;
use postrinch_mailtest::{Extension, ImapFault};
use postrinch_model::MessageChange;
use postrinch_sync::SyncError;

use super::changes_support::{change, count, expunge_copy, put, record, sync_agrees, uid_of};
use super::sync_support::{Fixture, fixture, fixture_without, mail};

/// Whether a folder on the server holds the message with this
/// `Message-ID`.
fn holds(f: &Fixture, folder: &str, header: &str) -> bool {
    let needle = format!("Message-ID: <{header}>");
    f.user
        .messages(folder)
        .unwrap()
        .iter()
        .any(|m| String::from_utf8_lossy(&m.body).contains(&needle))
}

fn archive() -> MessageChange {
    MessageChange::remove_label("INBOX")
}

#[tokio::test]
async fn deleting_a_label_never_counts_on_a_copy_that_was_not_made() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    for folder in ["L", "B"] {
        f.user.create_folder(folder).unwrap();
    }
    // x@x came to the inbox first, so a copy of it is made from there.
    let earlier = DateTime::parse_from_rfc3339("2026-01-01T09:00:00+00:00").unwrap();
    f.user
        .append("INBOX", mail(Some("x@x"), "x@x", "").internal_date(earlier))
        .unwrap();
    put(&f, "x@x", &["L"]);
    f.sync().await;
    // Another client expunges the inbox copy, and other mail arrives in B
    // as the label is added.
    expunge_copy(&f, "INBOX", "x@x");
    let user = f.user.clone();
    f.server.on_next("UID COPY", "", move || {
        user.append("B", mail(Some("y@x"), "y@x", "")).unwrap();
    });
    change(&f, &["x@x"], MessageChange::add_label(f.label_id("B"))).await;
    f.sync.delete_label(&f.me, &f.label_id("L")).await.unwrap();
    assert!(f.user.folder("L").is_err(), "the label is deleted");
    assert!(
        holds(&f, "B", "x@x") || holds(&f, "Archive", "x@x"),
        "x@x is still on the server"
    );
}

#[tokio::test]
async fn a_copy_not_found_is_never_made_again_in_a_folder_sync_cannot_read() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    for folder in ["Work", "Home", "Other"] {
        f.user.create_folder(folder).unwrap();
    }
    put(&f, "a@x", &["INBOX", "Home", "Other"]);
    f.sync().await;
    // Work has not been read whole for five passes in a row.
    f.server
        .fail_next_matching("SELECT", "\"Work\"", 5, ImapFault::No("Not now".into()));
    for _ in 0..5 {
        f.sync().await;
    }
    // Home's copy moves to Work and Other's is deleted; the listing that
    // would find the moved copy is refused.
    f.server
        .fail_next("UID FETCH", 1, ImapFault::No("Not now".into()));
    let refile = MessageChange {
        add_labels: vec![f.label_id("Work")],
        remove_labels: vec![f.label_id("Home"), f.label_id("Other")],
        ..MessageChange::default()
    };
    record(&f, &["a@x"], refile);
    let first = f.sync.send_outbox(&f.me).await;
    assert!(matches!(first, Err(SyncError::Server { .. })), "{first:?}");
    assert_eq!((count(&f, "Work"), count(&f, "Other")), (1, 1));
    for _ in 0..3 {
        let _ = f.sync.send_outbox(&f.me).await;
        assert_eq!(count(&f, "Work"), 1, "never copied twice");
    }
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 0);
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Home"), count(&f, "Other")),
        (1, 0, 0)
    );
}

#[tokio::test]
async fn copies_left_marked_are_expunged_once_the_other_mail_is_gone() {
    let f = fixture_without(&[Extension::Move, Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    put(&f, "c@x", &["INBOX"]);
    f.sync().await;
    // Another client marks b@x deleted and leaves it so: once sync has
    // seen it, ours are left marked beside it.
    let b = uid_of(&f, "INBOX", "b@x");
    f.user.add_flags("INBOX", b, &["\\Deleted"]).unwrap();
    record(&f, &["a@x"], archive());
    assert!(f.sync.send_outbox(&f.me).await.is_err(), "waits for sync");
    f.sync().await;
    f.sync().await;
    let left = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!(left.sent, 1);
    assert_eq!(f.server.command_count("EXPUNGE"), 0);
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (3, 1));
    // That client expunges its own; ours stay marked.
    f.user.expunge("INBOX", b).unwrap();
    f.sync().await;
    // The next send expunges ours left marked, before its own delete
    // there expunges c@x.
    let report = change(&f, &["c@x"], archive()).await;
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(f.server.command_count("EXPUNGE"), 2);
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (0, 2));
    sync_agrees(&f).await;
}

/// Work with a@x, which sync has read, and w@x, which came after and
/// whose headers the server never gives.
async fn with_silent_mail(f: &Fixture) -> String {
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(f, "a@x", &["Work"]);
    f.sync().await;
    let w = f.user.append("Work", mail(Some("w@x"), "w@x", "")).unwrap();
    f.user.withhold("Work", w, true).unwrap();
    f.label_id("Work")
}

#[tokio::test]
async fn deleting_a_label_saves_mail_whose_headers_never_come() {
    let f = fixture().await;
    let work = with_silent_mail(&f).await;
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert!(f.user.folder("Work").is_err(), "deleted at once");
    assert!(holds(&f, "Archive", "a@x") && holds(&f, "Archive", "w@x"));
    assert_eq!(count(&f, "Archive"), 2);
}

#[tokio::test]
async fn without_uidplus_a_label_whose_mail_gives_no_headers_is_kept_saying_why() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    let work = with_silent_mail(&f).await;
    let refused = f.sync.delete_label(&f.me, &work).await;
    assert!(
        matches!(&refused, Err(SyncError::Server { message, .. }) if message.contains("headers")),
        "{refused:?}"
    );
    assert_eq!(count(&f, "Work"), 2, "the folder and its mail stay");
}
