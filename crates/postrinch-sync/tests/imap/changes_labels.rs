//! Labels as folders: making, renaming and deleting one on the server,
//! and the next sync keeping the same label.

use postrinch_sync::SyncError;

use super::changes_support::{count, put, sync_agrees};
use super::sync_support::fixture;

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
