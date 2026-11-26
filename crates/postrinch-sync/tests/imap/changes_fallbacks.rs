//! Changing mail on servers without MOVE or UIDPLUS: a move is a copy, a
//! `\Deleted` mark and an expunge; copies are found by `Message-ID` when
//! the server does not say where they went; a plain `EXPUNGE` is sent
//! only when it removes nothing but ours.

use postrinch_mailtest::Extension;
use postrinch_model::MessageChange;
use postrinch_sync::SyncError;

use super::changes_support::{change, count, put, record, sync_agrees};
use super::sync_support::{fixture_without, mail};

fn archive() -> MessageChange {
    MessageChange::remove_label("INBOX")
}

#[tokio::test]
async fn without_move_a_move_is_copy_mark_and_uid_expunge() {
    let f = fixture_without(&[Extension::Move]).await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    change(&f, &["a@x"], archive()).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (1, 1));
    assert_eq!(f.server.command_count("UID MOVE"), 0);
    assert_eq!(f.server.command_count("UID COPY"), 1);
    assert_eq!(f.server.command_count("UID EXPUNGE"), 1);
    assert_eq!(f.labels("a@x"), [f.label_id("Archive")]);
    assert_eq!(f.labels("b@x"), ["INBOX"]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn without_uidplus_moved_copies_are_found_by_message_id() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    change(&f, &["a@x", "b@x"], archive()).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (0, 2));
    assert_eq!(f.labels("a@x"), [f.label_id("Archive")]);
    assert_eq!(f.labels("b@x"), [f.label_id("Archive")]);
    sync_agrees(&f).await;
    assert_eq!(f.message_count(), 2);
}

#[tokio::test]
async fn without_either_only_our_deleted_mail_is_expunged() {
    let f = fixture_without(&[Extension::Move, Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    change(&f, &["a@x"], archive()).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (1, 1));
    assert_eq!(f.server.command_count("EXPUNGE"), 1);
    sync_agrees(&f).await;
    // Another client marked b@x deleted: expunging would remove it too,
    // so nothing is copied and the change waits.
    let c = f.user.append("INBOX", mail(Some("c@x"), "C", "")).unwrap();
    f.sync().await;
    let b = f.user.messages("INBOX").unwrap()[0].uid;
    f.user.add_flags("INBOX", b, &["\\Deleted"]).unwrap();
    record(&f, &["c@x"], archive());
    let waits = f.sync.send_outbox(&f.me).await;
    assert!(matches!(waits, Err(SyncError::Server { .. })), "{waits:?}");
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (2, 1));
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 1);
    // Once that mail is gone, it goes through.
    f.user.expunge("INBOX", b).unwrap();
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!(report.sent, 1);
    assert!(f.user.messages("INBOX").unwrap().iter().all(|m| m.uid != c));
    assert_eq!(count(&f, "Archive"), 2);
}

#[tokio::test]
async fn a_message_without_message_id_is_left_to_sync_and_kept_once() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    f.user.append("INBOX", mail(None, "No id", "")).unwrap();
    f.sync().await;
    let ids = f.store.message_ids(&f.me).unwrap();
    f.store.change_messages(&f.me, &ids, &archive()).unwrap();
    f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (0, 1));
    f.sync().await;
    f.sync().await;
    let ids = f.store.message_ids(&f.me).unwrap();
    assert_eq!(ids.len(), 1, "kept once: {ids:?}");
    let message = f.store.message(&f.me, &ids[0]).unwrap().unwrap();
    assert_eq!(message.labels, [f.label_id("Archive")]);
}

#[tokio::test]
async fn a_message_in_several_folders_changes_in_each() {
    let f = fixture_without(&[Extension::Move]).await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    f.user.create_folder("Home").unwrap();
    put(&f, "a@x", &["INBOX", "Work", "Home"]);
    f.sync().await;
    let (work, home) = (f.label_id("Work"), f.label_id("Home"));
    // Out of Work and Home into Clients: one moves, the other goes.
    f.user.create_folder("Clients").unwrap();
    f.sync().await;
    let clients = f.label_id("Clients");
    let refile = MessageChange {
        add_labels: vec![clients.clone()],
        remove_labels: vec![work, home],
        ..MessageChange::default()
    };
    change(&f, &["a@x"], refile).await;
    assert_eq!(
        (
            count(&f, "INBOX"),
            count(&f, "Work"),
            count(&f, "Home"),
            count(&f, "Clients")
        ),
        (1, 0, 0, 1)
    );
    let mut labels = vec!["INBOX".to_owned(), clients];
    labels.sort();
    assert_eq!(f.labels("a@x"), labels);
    sync_agrees(&f).await;
}
