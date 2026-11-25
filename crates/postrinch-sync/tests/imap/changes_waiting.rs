//! Changes that wait: for sync to read a folder, for other mail to go, or
//! for the server. Waiting is never counted as a failed send, so a change
//! that waits is never given up on; it holds back only the later changes
//! to its messages; and a copy whose UID the server did not say is not
//! made again while it waits for sync. Other mail another client leaves
//! marked deleted holds a change back only until sync has seen it.

use postrinch_mailtest::{Extension, ImapFault};
use postrinch_model::MessageChange;
use postrinch_sync::SyncError;

use super::changes_support::{all_have, count, put, record, sync_agrees, uid_of};
use super::sync_support::{fixture, fixture_without, mail};

fn archive() -> MessageChange {
    MessageChange::remove_label("INBOX")
}

#[tokio::test]
async fn a_change_waiting_for_other_mail_to_go_is_never_given_up_on() {
    let f = fixture_without(&[Extension::Move, Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    let other = f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    f.sync().await;
    // Another client marked b@x deleted: a plain EXPUNGE would take it.
    f.user.add_flags("INBOX", other, &["\\Deleted"]).unwrap();
    record(&f, &["a@x"], archive());
    for _ in 0..8 {
        let waits = f.sync.send_outbox(&f.me).await;
        assert!(matches!(waits, Err(SyncError::Server { .. })), "{waits:?}");
        assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 1, "still waiting");
    }
    assert_eq!(f.store.outbox(&f.me).unwrap()[0].attempts, 0, "not counted");
    assert!(f.labels("a@x").is_empty(), "still shown archived");
    f.user.expunge("INBOX", other).unwrap();
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (0, 1));
}

#[tokio::test]
async fn waiting_for_the_server_is_not_counted_against_a_change() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    f.server
        .fail_next("UID MOVE", 4, ImapFault::DisconnectBefore);
    f.server
        .fail_next("UID MOVE", 1, ImapFault::Bad("Not now".into()));
    record(&f, &["a@x"], archive());
    for _ in 0..4 {
        let dropped = f.sync.send_outbox(&f.me).await;
        assert!(dropped.is_err(), "{dropped:?}");
    }
    // One declined send, the first that counts: tried again, not dropped.
    let declined = f.sync.send_outbox(&f.me).await;
    assert!(
        matches!(declined, Err(SyncError::Server { .. })),
        "{declined:?}"
    );
    let entry = &f.store.outbox(&f.me).unwrap()[0];
    assert_eq!((entry.attempts, entry.sent), (1, None));
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(count(&f, "Archive"), 1);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_copy_not_found_is_not_made_again_before_sync_finds_it() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    for folder in ["Work", "Home", "Other"] {
        f.user.create_folder(folder).unwrap();
    }
    put(&f, "a@x", &["INBOX", "Home", "Other"]);
    f.sync().await;
    let (work, home, other) = (f.label_id("Work"), f.label_id("Home"), f.label_id("Other"));
    // Home's copy moves to Work, and Other's is deleted. The server does
    // not say where the moved copy went, and does not answer the FETCH
    // that would find it: Other's copy stays, as it is not known that the
    // message is in Work.
    f.server
        .fail_next("UID FETCH", 1, ImapFault::No("Not now".into()));
    let refile = MessageChange {
        add_labels: vec![work],
        remove_labels: vec![home, other],
        ..MessageChange::default()
    };
    record(&f, &["a@x"], refile);
    let first = f.sync.send_outbox(&f.me).await;
    assert!(matches!(first, Err(SyncError::Server { .. })), "{first:?}");
    assert_eq!(
        (count(&f, "Home"), count(&f, "Work"), count(&f, "Other")),
        (0, 1, 1)
    );
    // Sent again before sync has read Work, it waits rather than copying
    // the message into Work again.
    let again = f.sync.send_outbox(&f.me).await;
    assert!(matches!(again, Err(SyncError::Server { .. })), "{again:?}");
    assert_eq!(count(&f, "Work"), 1, "not copied twice");
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 1);
    // Once sync has found it, the rest goes.
    f.sync().await;
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Work"), count(&f, "Other")),
        (1, 1, 0)
    );
    sync_agrees(&f).await;
}

#[tokio::test]
async fn mail_another_client_leaves_marked_deleted_holds_a_change_only_until_sync() {
    let f = fixture_without(&[Extension::Move, Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    // Another client marks b@x deleted and never expunges it (Thunderbird's
    // "mark as deleted"): a plain EXPUNGE would take it.
    let other = uid_of(&f, "INBOX", "b@x");
    f.user.add_flags("INBOX", other, &["\\Deleted"]).unwrap();
    record(&f, &["a@x"], archive());
    let waits = f.sync.send_outbox(&f.me).await;
    assert!(matches!(waits, Err(SyncError::Server { .. })), "{waits:?}");
    // Once sync has seen it marked (and, a pass later, forgotten b@x, which
    // counts as gone), ours are marked too and left for that client's
    // expunge: the change is made, and nothing is expunged.
    f.sync().await;
    f.sync().await;
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(f.server.command_count("EXPUNGE"), 0, "nothing expunged");
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (2, 1));
    assert!(
        all_have(&f, "INBOX", "\\Deleted"),
        "ours marked, not expunged"
    );
    assert_eq!(f.labels("a@x"), [f.label_id("Archive")]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_change_that_waits_holds_back_only_later_changes_to_its_messages() {
    let f = fixture_without(&[Extension::Move, Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "c@x", &["Work"]);
    let other = f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    f.sync().await;
    f.user.add_flags("INBOX", other, &["\\Deleted"]).unwrap();
    // Archiving a@x waits for b@x to go; starring a@x waits behind it, and
    // starring c@x, in another folder, goes.
    record(&f, &["a@x"], archive());
    record(&f, &["a@x"], MessageChange::starred(true));
    record(&f, &["c@x"], MessageChange::starred(true));
    let waits = f.sync.send_outbox(&f.me).await;
    assert!(matches!(waits, Err(SyncError::Server { .. })), "{waits:?}");
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 2, "a@x's two wait");
    assert!(all_have(&f, "Work", "\\Flagged"), "c@x starred");
    let a = f.user.message("INBOX", uid_of(&f, "INBOX", "a@x")).unwrap();
    assert!(!a.has_flag("\\Flagged"), "a@x waits");
    f.user.expunge("INBOX", other).unwrap();
    // Sync reads b@x gone (a pass to find it so, and one to forget it).
    f.sync().await;
    f.sync().await;
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (2, 0));
    assert!(
        all_have(&f, "Archive", "\\Flagged"),
        "in order: archived, then starred"
    );
    sync_agrees(&f).await;
}
