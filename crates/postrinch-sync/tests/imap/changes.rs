//! Changing mail on a server offering every extension: each kind of
//! change reaches the server's copies, and the next sync agrees.

use postrinch_mailtest::SpecialUse;
use postrinch_model::MessageChange;

use super::changes_support::{all_have, change, count, put, sync_agrees};
use super::sync_support::fixture;

#[tokio::test]
async fn read_unread_and_star_reach_every_copy() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX", "Work"]);
    f.sync().await;
    let report = change(&f, &["a@x"], MessageChange::read()).await;
    assert_eq!((report.sent, report.refused.len()), (1, 0));
    assert!(all_have(&f, "INBOX", "\\Seen") && all_have(&f, "Work", "\\Seen"));
    sync_agrees(&f).await;
    change(&f, &["a@x"], MessageChange::unread()).await;
    assert!(!f.user.messages("INBOX").unwrap()[0].has_flag("\\Seen"));
    assert!(!f.user.messages("Work").unwrap()[0].has_flag("\\Seen"));
    change(&f, &["a@x"], MessageChange::starred(true)).await;
    assert!(all_have(&f, "INBOX", "\\Flagged") && all_have(&f, "Work", "\\Flagged"));
    sync_agrees(&f).await;
    let stored = f.message("a@x").unwrap();
    assert!(stored.flags.unread && stored.flags.starred);
    assert_eq!(f.sync.waiting(&f.me).await.unwrap(), 0);
}

#[tokio::test]
async fn archiving_moves_mail_to_the_archive_folder() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    put(&f, "b@x", &["INBOX"]);
    f.sync().await;
    change(&f, &["a@x", "b@x"], MessageChange::remove_label("INBOX")).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "Archive")), (0, 2));
    assert_eq!(
        f.server.command_count("UID MOVE"),
        1,
        "one command for both"
    );
    assert_eq!(f.labels("a@x"), [f.label_id("Archive")]);
    sync_agrees(&f).await;
    assert_eq!(f.message_count(), 2);
}

#[tokio::test]
async fn archiving_without_an_archive_folder_makes_one() {
    let f = fixture().await;
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    change(&f, &["a@x"], MessageChange::remove_label("INBOX")).await;
    assert_eq!(count(&f, "Archive"), 1);
    assert!(f.user.folder("Archive").unwrap().subscribed);
    assert_eq!(f.labels("a@x"), [f.label_id("Archive")]);
    let id = f.label_id("Archive");
    sync_agrees(&f).await;
    assert_eq!(f.label_id("Archive"), id, "sync keeps the label made");
}

#[tokio::test]
async fn trash_takes_every_copy_and_restoring_brings_it_back() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX", "Work"]);
    f.sync().await;
    change(&f, &["a@x"], MessageChange::add_label("TRASH")).await;
    assert_eq!(
        (count(&f, "INBOX"), count(&f, "Work"), count(&f, "Trash")),
        (0, 0, 1)
    );
    assert_eq!(f.labels("a@x"), ["TRASH"]);
    sync_agrees(&f).await;
    change(&f, &["a@x"], MessageChange::remove_label("TRASH")).await;
    assert_eq!((count(&f, "Trash"), count(&f, "INBOX")), (0, 1));
    assert_eq!(f.labels("a@x"), ["INBOX"]);
    sync_agrees(&f).await;
    assert_eq!(f.message_count(), 1);
}

#[tokio::test]
async fn spam_and_not_spam() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    change(&f, &["a@x"], MessageChange::add_label("SPAM")).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "Junk")), (0, 1));
    assert_eq!(f.labels("a@x"), ["SPAM"]);
    sync_agrees(&f).await;
    let not_spam = MessageChange {
        add_labels: vec!["INBOX".into()],
        remove_labels: vec!["SPAM".into()],
        ..MessageChange::default()
    };
    change(&f, &["a@x"], not_spam).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "Junk")), (1, 0));
    assert_eq!(f.labels("a@x"), ["INBOX"]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn a_label_is_a_copy_in_its_folder() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.user.create_folder("Work").unwrap();
    put(&f, "a@x", &["INBOX"]);
    f.sync().await;
    let work = f.label_id("Work");
    change(&f, &["a@x"], MessageChange::add_label(&work)).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "Work")), (1, 1));
    let mut both = vec!["INBOX".to_owned(), work.clone()];
    both.sort();
    assert_eq!(f.labels("a@x"), both);
    sync_agrees(&f).await;
    change(&f, &["a@x"], MessageChange::remove_label(&work)).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "Work")), (1, 0));
    assert_eq!(f.labels("a@x"), ["INBOX"]);
    sync_agrees(&f).await;
}

#[tokio::test]
async fn all_mail_is_left_alone_as_gmail_keeps_it() {
    let f = fixture().await;
    f.user
        .create_special_folder("[Gmail]/All Mail", SpecialUse::All)
        .unwrap();
    f.user
        .create_special_folder("[Gmail]/Trash", SpecialUse::Trash)
        .unwrap();
    put(&f, "a@x", &["INBOX", "[Gmail]/All Mail"]);
    f.sync().await;
    let all = f.label_id("[Gmail]/All Mail");
    // Archiving removes the inbox copy: Gmail drops the label.
    change(&f, &["a@x"], MessageChange::remove_label("INBOX")).await;
    assert_eq!((count(&f, "INBOX"), count(&f, "[Gmail]/All Mail")), (0, 1));
    assert_eq!(f.labels("a@x"), std::slice::from_ref(&all));
    sync_agrees(&f).await;
    // Trash moves it out of All Mail, as Gmail does.
    change(&f, &["a@x"], MessageChange::add_label("TRASH")).await;
    assert_eq!(
        (count(&f, "[Gmail]/All Mail"), count(&f, "[Gmail]/Trash")),
        (0, 1)
    );
    sync_agrees(&f).await;
}
