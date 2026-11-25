//! IMAP sync's folders and identities against the fake server: labels for
//! folders, special uses and the person's choices, Gmail's All Mail,
//! renames and deletions, one message in several folders, header-less
//! mail and threads across folders.

use postrinch_mailtest::{Extension, ImapConfig, SpecialUse};
use postrinch_model::{FolderRole, FolderRoles, LabelKind, SystemLabel};
use postrinch_sync::imap::message_id_without_header;

use super::sync_support::{Fixture, fixture, fixture_with, fixture_without, mail};

#[tokio::test]
async fn special_folders_are_the_system_labels() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    f.sync().await;
    for (name, id) in [
        ("INBOX", "INBOX"),
        ("Sent", "SENT"),
        ("Drafts", "DRAFT"),
        ("Trash", "TRASH"),
        ("Junk", "SPAM"),
    ] {
        assert_eq!(f.label_id(name), id, "{name}");
    }
    let work = f.label_id("Work");
    assert!(work.starts_with('f') && work.len() == 13, "{work}");
    let archive = f
        .store
        .imap_folder(&f.me, &f.label_id("Archive"))
        .unwrap()
        .unwrap();
    assert_eq!(archive.role, Some(FolderRole::Archive));
    // Ids are kept from pass to pass.
    f.sync().await;
    assert_eq!(f.label_id("Work"), work);
}

#[tokio::test]
async fn the_persons_choice_of_folder_wins() {
    let roles = FolderRoles {
        trash: Some("Bin".into()),
        ..FolderRoles::default()
    };
    let f = fixture_with(ImapConfig::new(), roles).await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Bin").unwrap();
    f.sync().await;
    assert_eq!(f.label_id("Bin"), "TRASH");
    let trash = f.label_id("Trash");
    assert!(trash.starts_with('f'));
    let label = f
        .store
        .labels(&f.me)
        .unwrap()
        .into_iter()
        .find(|l| l.provider_id == "TRASH")
        .unwrap();
    assert_eq!(label.kind, LabelKind::System(SystemLabel::Trash));
}

#[tokio::test]
async fn gmail_all_mail_is_the_archive_and_one_message_has_both_labels() {
    // Gmail says so with X-GM-EXT-1; elsewhere `\All` is a view, not synced.
    let f = fixture_with(ImapConfig::new().gmail(true), FolderRoles::default()).await;
    f.user.create_noselect_folder("[Gmail]").unwrap();
    f.user
        .create_special_folder("[Gmail]/All Mail", SpecialUse::All)
        .unwrap();
    f.user
        .create_special_folder("[Gmail]/Starred", SpecialUse::Flagged)
        .unwrap();
    f.user
        .create_special_folder("[Gmail]/Important", SpecialUse::Important)
        .unwrap();
    f.user
        .create_special_folder("[Gmail]/Sent Mail", SpecialUse::Sent)
        .unwrap();
    for folder in [
        "INBOX",
        "[Gmail]/All Mail",
        "[Gmail]/Starred",
        "[Gmail]/Important",
    ] {
        f.user
            .append(folder, mail(Some("g@x"), "Gmail", "").flags(&["\\Flagged"]))
            .unwrap();
    }
    f.sync().await;
    assert_eq!(
        f.label_names(),
        ["INBOX", "[Gmail]/All Mail", "[Gmail]/Sent Mail"]
    );
    let all = f.label_id("[Gmail]/All Mail");
    let folder = f.store.imap_folder(&f.me, &all).unwrap().unwrap();
    assert_eq!(folder.role, Some(FolderRole::Archive));
    let mut want = vec!["INBOX".to_owned(), all.clone()];
    want.sort();
    assert_eq!(f.labels("g@x"), want);
    assert!(f.message("g@x").unwrap().flags.starred);
    assert_eq!(f.message_count(), 1);
    // Archiving elsewhere: the inbox copy goes, the message stays.
    f.user.expunge("INBOX", 1).unwrap();
    f.sync().await;
    assert_eq!(f.labels("g@x"), [all]);
}

#[tokio::test]
async fn copies_in_two_folders_are_one_message() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user
        .append(
            "Sent",
            mail(Some("self@x"), "To myself", "").flags(&["\\Seen"]),
        )
        .unwrap();
    f.user
        .append("INBOX", mail(Some("self@x"), "To myself", ""))
        .unwrap();
    f.sync().await;
    assert_eq!(f.message_count(), 1);
    assert_eq!(f.labels("self@x"), ["INBOX", "SENT"]);
    let id = f.id("self@x");
    assert_eq!(f.store.imap_message_places(&f.me, &id).unwrap().len(), 2);
    // Unread while any copy is.
    assert!(f.message("self@x").unwrap().flags.unread);
    f.user.add_flags("INBOX", 1, &["\\Seen"]).unwrap();
    f.sync().await;
    assert!(!f.message("self@x").unwrap().flags.unread);
    // The inbox copy goes: one label left. The last goes: removed.
    f.user.expunge("INBOX", 1).unwrap();
    f.sync().await;
    assert_eq!(f.labels("self@x"), ["SENT"]);
    f.user.expunge("Sent", 1).unwrap();
    f.sync().await;
    f.sync().await;
    assert!(f.message("self@x").is_none());
}

#[tokio::test]
async fn mail_moved_by_another_client_keeps_its_body() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    let uid = f
        .user
        .append("INBOX", mail(Some("mv@x"), "Moved", ""))
        .unwrap();
    f.sync().await;
    f.backfill().await;
    f.user
        .append("Archive", mail(Some("mv@x"), "Moved", ""))
        .unwrap();
    f.user.expunge("INBOX", uid).unwrap();
    let report = f.sync().await;
    assert_eq!(report.removed, 0);
    let moved = f.message("mv@x").unwrap();
    assert!(moved.downloaded);
    assert_eq!(moved.labels, [f.label_id("Archive")]);
}

#[tokio::test]
async fn a_renamed_folder_keeps_its_label_by_mailbox_id() {
    a_renamed_folder_keeps_its_label(fixture().await).await;
}

#[tokio::test]
async fn a_renamed_folder_keeps_its_label_by_uidvalidity() {
    a_renamed_folder_keeps_its_label(fixture_without(&[Extension::ObjectId]).await).await;
}

async fn a_renamed_folder_keeps_its_label(f: Fixture) {
    f.user.create_folder("Work").unwrap();
    f.user
        .append("Work", mail(Some("w@x"), "Work", ""))
        .unwrap();
    f.sync().await;
    f.backfill().await;
    let old = f.label_id("Work");
    f.user.rename_folder("Work", "Projects").unwrap();
    let report = f.sync().await;
    assert!(!report.full, "a renamed folder is not read again");
    assert_eq!(f.label_names(), ["INBOX", "Projects"]);
    assert_eq!(f.label_id("Projects"), old);
    let w = f.message("w@x").unwrap();
    assert_eq!(w.labels, std::slice::from_ref(&old));
    assert!(w.downloaded);
    let state = f.store.imap_folder(&f.me, &old).unwrap().unwrap();
    assert_eq!(state.name, "Projects");
    f.sync().await;
    assert_eq!(f.label_id("Projects"), old);
    assert!(f.message("w@x").unwrap().downloaded);
}

#[tokio::test]
async fn a_deleted_folder_takes_its_mail() {
    let f = fixture().await;
    f.user.create_folder("Old").unwrap();
    f.user.append("Old", mail(Some("o@x"), "Old", "")).unwrap();
    f.user
        .append("INBOX", mail(Some("keep@x"), "Keep", ""))
        .unwrap();
    f.sync().await;
    f.user.delete_folder("Old").unwrap();
    // Missing from one LIST, it may be back: nothing goes.
    assert_eq!(f.sync().await.removed, 0);
    assert_eq!(f.label_names(), ["INBOX", "Old"]);
    // Missing from two, it is gone, and its mail waits one more pass.
    assert_eq!(f.sync().await.removed, 0);
    assert_eq!(f.label_names(), ["INBOX"]);
    assert_eq!(f.sync().await.removed, 1);
    assert!(f.message("o@x").is_none());
    assert!(f.message("keep@x").is_some());
}

#[tokio::test]
async fn modified_utf7_names_are_decoded() {
    let f = fixture().await;
    f.user.create_folder("Entw&APw-rfe").unwrap();
    f.user
        .create_folder("&BB4EQgQ,BEAEMAQyBDsENQQ9BD0ESwQ1-/Tom &- Jerry")
        .unwrap();
    f.user
        .append("Entw&APw-rfe", mail(Some("u@x"), "Umlaut", ""))
        .unwrap();
    f.sync().await;
    let names = f.label_names();
    assert!(names.contains(&"Entwürfe".to_owned()), "{names:?}");
    assert!(
        names.contains(&"Отправленные/Tom & Jerry".to_owned()),
        "{names:?}"
    );
    assert_eq!(f.labels("u@x"), [f.label_id("Entwürfe")]);
    let folder = f
        .store
        .imap_folder(&f.me, &f.label_id("Entwürfe"))
        .unwrap()
        .unwrap();
    assert_eq!(folder.name, "Entw&APw-rfe");
}

#[tokio::test]
async fn a_message_without_a_message_id_is_named_by_its_place() {
    let f = fixture().await;
    let uid = f.user.append("INBOX", mail(None, "Automated", "")).unwrap();
    f.sync().await;
    let uidvalidity = f.user.folder("INBOX").unwrap().uid_validity;
    let id = message_id_without_header(uidvalidity, "INBOX", uid);
    let stored = f.store.message(&f.me, &id).unwrap().expect("stored");
    assert_eq!(stored.subject(), "Automated");
    assert!(stored.thread.0.starts_with('t'));
    assert_eq!(f.message_count(), 1);
}

#[tokio::test]
async fn threads_join_across_folders() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user
        .append("Sent", mail(Some("root@x"), "Plans", ""))
        .unwrap();
    f.user
        .append(
            "INBOX",
            mail(
                Some("reply@x"),
                "Re: Plans",
                "In-Reply-To: <root@x>\r\nReferences: <root@x>\r\n",
            ),
        )
        .unwrap();
    f.user
        .append("INBOX", mail(Some("other@x"), "Unrelated", ""))
        .unwrap();
    f.sync().await;
    let root = f.message("root@x").unwrap();
    let reply = f.message("reply@x").unwrap();
    assert_eq!(root.thread, reply.thread);
    assert_ne!(f.message("other@x").unwrap().thread, root.thread);
}
