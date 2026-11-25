//! A copy that only seems to keep a message never lets another go: a
//! `\All` folder that is a view of the others (anywhere but Gmail), or a
//! copy elsewhere that another client deleted or whose folder's
//! UIDVALIDITY changed since sync read it, or that the server will not
//! say is there. Nor is mail filed in a view.

use postrinch_mailtest::{ImapConfig, ImapFault, SpecialUse};
use postrinch_model::{FolderRoles, MessageChange};
use postrinch_sync::SyncError;

use super::changes_support::{change, count, put, record, sync_agrees, uid_of};
use super::sync_support::{Fixture, fixture, fixture_with};

fn archive() -> MessageChange {
    MessageChange::remove_label("INBOX")
}

const VIEW: &str = "virtual/All";

/// Elsewhere than Gmail, with a `\All` view of every folder and no
/// archive folder.
async fn with_a_view() -> Fixture {
    let f = fixture().await;
    f.user.create_special_folder(VIEW, SpecialUse::All).unwrap();
    f
}

#[tokio::test]
async fn archiving_beside_a_view_of_all_mail_files_it_in_a_real_archive() {
    let f = with_a_view().await;
    put(&f, "a@x", &["INBOX", VIEW]);
    f.sync().await;
    assert!(
        !f.label_names().contains(&VIEW.to_owned()),
        "a view is not synced: {:?}",
        f.label_names()
    );
    let report = change(&f, &["a@x"], archive()).await;
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(count(&f, "INBOX"), 0);
    assert_eq!(count(&f, "Archive"), 1, "moved to a real archive");
    assert_eq!(count(&f, VIEW), 1, "the view is not touched");
    assert_eq!(f.labels("a@x"), [f.label_id("Archive")]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn deleting_a_label_beside_a_view_of_all_mail_saves_its_mail() {
    let f = with_a_view().await;
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["Work", VIEW]);
    f.sync().await;
    let work = f.label_id("Work");
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert!(f.user.folder("Work").is_err());
    assert_eq!(count(&f, "Archive"), 1, "saved in a real archive");
    assert_eq!(f.labels("a@x"), [f.label_id("Archive")]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn on_gmail_all_mail_keeps_what_a_deleted_label_held() {
    let f = fixture_with(ImapConfig::new().gmail(true), FolderRoles::default()).await;
    f.user
        .create_special_folder("[Gmail]/All Mail", SpecialUse::All)
        .unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["Work", "[Gmail]/All Mail"]);
    f.sync().await;
    let all = f.label_id("[Gmail]/All Mail");
    let work = f.label_id("Work");
    f.sync.delete_label(&f.me, &work).await.unwrap();
    assert!(f.user.folder("Work").is_err());
    assert_eq!(f.server.command_count("UID COPY"), 0, "All Mail has it");
    assert_eq!(f.labels("a@x"), [all]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_copy_another_client_deleted_does_not_keep_the_message() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX", "Work"]);
    f.sync().await;
    let uid = f.user.messages("Work").unwrap()[0].uid;
    f.user.expunge("Work", uid).unwrap();
    let report = change(&f, &["a@x"], archive()).await;
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Work"), count(&f, "Archive")),
        (0, 0, 1),
        "moved to the archive rather than deleted"
    );
    assert_eq!(f.labels("a@x"), [f.label_id("Archive")]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_copy_whose_folder_changed_since_sync_does_not_keep_the_message() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX", "Work"]);
    f.sync().await;
    // The UIDs sync read in Work name other messages now: its copy neither
    // keeps the message nor is evidence that there is none, so the inbox
    // copy is neither deleted nor filed in the archive.
    f.user.set_uid_validity("Work", 99).unwrap();
    record(&f, &["a@x"], archive());
    let waits = f.sync.send_outbox(&f.me).await;
    assert!(matches!(waits, Err(SyncError::Server { .. })), "{waits:?}");
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Work"), count(&f, "Archive")),
        (1, 1, 0)
    );
    // Once sync has read Work again, its copy keeps the message.
    f.sync().await;
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Work"), count(&f, "Archive")),
        (0, 1, 0)
    );
    f.sync().await;
    assert_eq!(f.labels("a@x"), [f.label_id("Work")]);
    assert_eq!(f.message_count(), 1);
}

#[tokio::test]
async fn mail_is_not_filed_in_a_folder_that_has_become_a_view() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder(VIEW).unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    let view = f.label_id(VIEW);
    // The folder is a view of all mail now, its label kept from before.
    f.user.delete_folder(VIEW).unwrap();
    f.user.create_special_folder(VIEW, SpecialUse::All).unwrap();
    let report = change(&f, &["a@x"], MessageChange::add_label(view)).await;
    assert_eq!((report.sent, report.refused.len()), (0, 1));
    assert!(report.refused[0].contains("a view"), "{:?}", report.refused);
    assert_eq!(count(&f, VIEW), 0, "nothing copied into the view");
    assert_eq!(f.labels("a@x"), ["INBOX"]);
}

#[tokio::test]
async fn a_copy_the_server_will_not_say_is_there_does_not_keep_the_message() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX", "Work"]);
    f.sync().await;
    // The inbox copy may be gone: Work's is neither deleted nor filed.
    f.server.fail_next_matching(
        "UID SEARCH",
        "UNDELETED",
        1,
        ImapFault::No("Not now".into()),
    );
    record(
        &f,
        &["a@x"],
        MessageChange::remove_label(f.label_id("Work")),
    );
    let waits = f.sync.send_outbox(&f.me).await;
    assert!(matches!(waits, Err(SyncError::Server { .. })), "{waits:?}");
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Work"), count(&f, "Archive")),
        (1, 1, 0)
    );
    let report = f.sync.send_outbox(&f.me).await.unwrap();
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Work"), count(&f, "Archive")),
        (1, 0, 0)
    );
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_copy_deleted_just_before_the_deletes_does_not_keep_the_message() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    for folder in ["Work", "Home"] {
        f.user.create_folder(folder).unwrap();
    }
    put(&f, "a@x", &["INBOX", "Work", "Home"]);
    f.sync().await;
    // The inbox copy is found, then deleted by another client as the
    // first of the other two goes.
    let inbox = uid_of(&f, "INBOX", "a@x");
    let user = f.user.clone();
    f.server.on_next("UID STORE", "", move || {
        user.expunge("INBOX", inbox).unwrap();
    });
    let out = MessageChange {
        remove_labels: vec![f.label_id("Work"), f.label_id("Home")],
        ..MessageChange::default()
    };
    record(&f, &["a@x"], out);
    let waits = f.sync.send_outbox(&f.me).await;
    assert!(matches!(waits, Err(SyncError::Server { .. })), "{waits:?}");
    assert_eq!(count(&f, "INBOX"), 0);
    assert_eq!(count(&f, "Work") + count(&f, "Home"), 1, "one copy is left");
}
