//! Labels as folders: making, renaming and deleting one on the server,
//! and the next sync keeping the same label.

use postrinch_mailtest::{Extension, ImapFault};
use postrinch_sync::SyncError;

use super::changes_support::{count, put, sync_agrees};
use super::sync_support::{fixture, fixture_without, mail};

#[tokio::test]
async fn a_label_made_is_a_folder_and_sync_keeps_its_id() {
    let f = fixture().await;
    f.sync().await;
    let made = f
        .sync
        .create_label(&f.me, "Projects/Entwürfe")
        .await
        .unwrap();
    assert_eq!(made.name, "Projects/Entwürfe");
    let folder = f.user.folder("Projects/Entw&APw-rfe").unwrap();
    assert!(folder.subscribed);
    assert_eq!(f.label_id("Projects/Entwürfe"), made.provider_id);
    sync_agrees(&f).await;
    assert_eq!(f.label_id("Projects/Entwürfe"), made.provider_id);
    // Making it again finds the one there.
    let again = f
        .sync
        .create_label(&f.me, "Projects/Entwürfe")
        .await
        .unwrap();
    assert_eq!(again.provider_id, made.provider_id);
}

#[tokio::test]
async fn renaming_a_label_keeps_its_id_and_its_mail() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.user.create_folder("Work/Old").unwrap();
    put(&f, "a@x", &["Work"]);
    f.sync().await;
    let (work, old) = (f.label_id("Work"), f.label_id("Work/Old"));
    let renamed = f.sync.rename_label(&f.me, &work, "Clients").await.unwrap();
    assert_eq!(
        (renamed.provider_id.as_str(), renamed.name.as_str()),
        (work.as_str(), "Clients")
    );
    assert!(f.user.folder("Work").is_err());
    assert_eq!(count(&f, "Clients"), 1);
    assert_eq!(
        f.label_id("Clients/Old"),
        old,
        "the folders under it follow"
    );
    assert_eq!(f.labels("a@x"), std::slice::from_ref(&work));
    sync_agrees(&f).await;
    assert_eq!(f.label_id("Clients"), work);
}

#[tokio::test]
async fn deleting_a_label_keeps_its_mail_in_the_archive() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "only@x", &["Work"]);
    put(&f, "both@x", &["INBOX", "Work"]);
    f.sync().await;
    let work = f.label_id("Work");
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert!(f.user.folder("Work").is_err());
    assert_eq!((count(&f, "Archive"), count(&f, "INBOX")), (1, 1));
    assert!(!f.label_names().contains(&"Work".to_owned()));
    assert_eq!(f.labels("only@x"), [f.label_id("Archive")]);
    assert_eq!(f.labels("both@x"), ["INBOX"]);
    sync_agrees(&f).await;
    assert_eq!(f.message_count(), 2);
}

#[tokio::test]
async fn the_special_folders_are_not_renamed_or_deleted() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.sync().await;
    let renamed = f.sync.rename_label(&f.me, "INBOX", "Post").await;
    assert!(
        matches!(renamed, Err(SyncError::Server { .. })),
        "{renamed:?}"
    );
    let archive = f.label_id("Archive");
    let deleted = f.sync.delete_label(&f.me, &archive).await;
    assert!(
        matches!(deleted, Err(SyncError::Server { .. })),
        "{deleted:?}"
    );
    assert!(f.user.folder("Archive").is_ok());
    assert_eq!(
        f.server.command_count("RENAME") + f.server.command_count("DELETE"),
        0
    );
}

#[tokio::test]
async fn deleting_a_label_whose_folder_changed_since_sync_saves_its_mail() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "only@x", &["Work"]);
    f.sync().await;
    // The UIDs sync read name other messages now.
    f.user.set_uid_validity("Work", 99).unwrap();
    let work = f.label_id("Work");
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert!(f.user.folder("Work").is_err());
    assert_eq!(
        count(&f, "Archive"),
        1,
        "saved, not deleted with the folder"
    );
    f.sync().await;
    assert_eq!(f.labels("only@x"), [f.label_id("Archive")]);
    assert_eq!(f.message_count(), 1);
}

#[tokio::test]
async fn deleting_a_label_saves_mail_sync_has_not_read() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "only@x", &["Work"]);
    f.sync().await;
    put(&f, "new@x", &["Work"]);
    // And mail arriving while the folder's mail is being saved.
    let user = f.user.clone();
    f.server.on_next("UID COPY", "", move || {
        user.append("Work", mail(Some("late@x"), "late@x", ""))
            .unwrap();
    });
    let work = f.label_id("Work");
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert!(f.user.folder("Work").is_err());
    assert_eq!(count(&f, "Archive"), 3, "everything the folder held");
    f.sync().await;
    for header in ["only@x", "new@x", "late@x"] {
        assert_eq!(f.labels(header), [f.label_id("Archive")], "{header}");
    }
    assert_eq!(f.message_count(), 3);
}

#[tokio::test]
async fn deleting_a_label_saves_mail_whose_other_copy_is_gone() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "both@x", &["INBOX", "Work"]);
    f.sync().await;
    let uid = f.user.messages("INBOX").unwrap()[0].uid;
    f.user.expunge("INBOX", uid).unwrap();
    let work = f.label_id("Work");
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (0, 1));
    assert_eq!(f.labels("both@x"), [f.label_id("Archive")]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn folders_of_the_persons_under_the_inbox_do_not_put_new_labels_there() {
    let f = fixture().await;
    f.user.create_folder("INBOX/Work").unwrap();
    f.sync().await;
    let made = f.sync.create_label(&f.me, "Projects").await.unwrap();
    assert_eq!(made.name, "Projects");
    assert!(f.user.folder("Projects").is_ok());
    assert!(f.user.folder("INBOX/Projects").is_err());
}

#[tokio::test]
async fn deleting_a_label_waits_when_its_folder_changes_while_its_mail_is_saved() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "kept@x", &["INBOX", "Work"]);
    f.sync().await;
    put(&f, "new@x", &["Work"]);
    // While the inbox is searched for kept@x's other copy, Work's UIDs
    // come to name other messages.
    let user = f.user.clone();
    f.server.on_next("SELECT", "INBOX", move || {
        user.set_uid_validity("Work", 99).unwrap();
    });
    let work = f.label_id("Work");
    let waits = f.sync.delete_label(&f.me, &work).await;
    assert!(matches!(waits, Err(SyncError::Server { .. })), "{waits:?}");
    assert!(f.user.folder("Work").is_ok(), "the folder stays");
    assert_eq!(
        count(&f, "Archive"),
        0,
        "nothing copied by UIDs read before"
    );
    // Once sync has read the folder again, its mail is saved.
    f.sync().await;
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert!(f.user.folder("Work").is_err());
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (1, 1));
    f.sync().await;
    assert_eq!(f.labels("new@x"), [f.label_id("Archive")]);
    assert_eq!(f.labels("kept@x"), ["INBOX"]);
}

#[tokio::test]
async fn deleting_a_label_again_copies_nothing_twice() {
    let f = fixture_without(&[Extension::Uidplus]).await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "only@x", &["Work"]);
    f.sync().await;
    put(&f, "new@x", &["Work"]);
    // Where the copies went is not found (the server will not list the
    // archive's new mail), so the folder is not deleted the first time; the
    // second finds them in the archive and copies nothing.
    f.server
        .fail_next_matching("UID FETCH", ":*", 1, ImapFault::No("Not now".into()));
    let work = f.label_id("Work");
    let first = f.sync.delete_label(&f.me, &work).await;
    assert!(matches!(first, Err(SyncError::Server { .. })), "{first:?}");
    assert_eq!(count(&f, "Archive"), 2);
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert!(f.user.folder("Work").is_err());
    assert_eq!(count(&f, "Archive"), 2, "nothing copied twice");
    f.sync().await;
    for header in ["only@x", "new@x"] {
        assert_eq!(f.labels(header), [f.label_id("Archive")], "{header}");
    }
    assert_eq!(f.message_count(), 2);
}
