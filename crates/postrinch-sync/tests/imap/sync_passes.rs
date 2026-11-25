//! Only a complete pass removes mail: a folder the server would not open,
//! one LIST left out once, a pass cut short, and mail moved into a folder
//! the pass had already read all keep a message and its body. And a
//! folder's role moving to another folder, end to end.

use postrinch_mailtest::{ImapFault, SpecialUse};
use postrinch_model::SystemLabel;

use super::sync_support::{Fixture, fixture, mail};

/// A message `mv@x` in the inbox, synced and downloaded, and a folder
/// Work, synced, to move it to.
async fn moved_from_the_inbox_to_work() -> (Fixture, String) {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    let uid = f
        .user
        .append("INBOX", mail(Some("mv@x"), "Moved", ""))
        .unwrap();
    f.sync().await;
    f.backfill().await;
    assert!(f.message("mv@x").unwrap().downloaded);
    f.user
        .append("Work", mail(Some("mv@x"), "Moved", ""))
        .unwrap();
    f.user.expunge("INBOX", uid).unwrap();
    let work = f.label_id("Work");
    (f, work)
}

fn body_downloads(f: &Fixture) -> usize {
    f.server
        .commands()
        .iter()
        .filter(|c| c.contains("BODY.PEEK[]"))
        .count()
}

#[tokio::test]
async fn a_refused_select_of_the_destination_keeps_a_moved_message() {
    let (f, work) = moved_from_the_inbox_to_work().await;
    let downloads = body_downloads(&f);
    f.server.fail_next_matching(
        "SELECT",
        "\"Work\"",
        1,
        // Refused, not busy (a busy server stops the pass instead).
        ImapFault::No("[SERVERBUG] Cannot open the mailbox".into()),
    );
    let report = f.sync().await;
    assert_eq!(report.removed, 0);
    let moved = f.message("mv@x").expect("kept while Work was not read");
    assert!(moved.downloaded);
    f.sync().await;
    let moved = f.message("mv@x").unwrap();
    assert_eq!(moved.labels, [work]);
    assert!(moved.downloaded, "the body was kept");
    f.backfill().await;
    assert_eq!(body_downloads(&f), downloads, "nothing downloaded again");
}

#[tokio::test]
async fn a_folder_left_out_of_list_once_keeps_its_mail_and_label() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.user
        .append("Work", mail(Some("w@x"), "Work", ""))
        .unwrap();
    f.sync().await;
    f.backfill().await;
    let work = f.label_id("Work");
    f.user.hide_folder("Work", true).unwrap();
    assert_eq!(f.sync().await.removed, 0);
    assert_eq!(f.label_id("Work"), work);
    f.user.hide_folder("Work", false).unwrap();
    assert_eq!(f.sync().await.removed, 0);
    assert_eq!(f.sync().await.removed, 0);
    assert_eq!(f.label_id("Work"), work);
    let w = f.message("w@x").unwrap();
    assert_eq!(w.labels, std::slice::from_ref(&work));
    assert!(w.downloaded);
    assert!(f.store.imap_folder(&f.me, &work).unwrap().is_some());
}

#[tokio::test]
async fn an_interrupted_pass_keeps_pending_messages() {
    let (f, work) = moved_from_the_inbox_to_work().await;
    f.server
        .fail_next_matching("SELECT", "\"Work\"", 1, ImapFault::DisconnectBefore);
    let quiet: postrinch_sync::google::ProgressFn = std::sync::Arc::new(|_| {});
    let cut = f.sync.sync(&f.me, quiet).await;
    assert!(cut.is_err(), "the pass was cut short: {cut:?}");
    let moved = f.message("mv@x").expect("kept after a pass cut short");
    assert!(moved.downloaded);
    assert_eq!(f.sync().await.removed, 0);
    let moved = f.message("mv@x").unwrap();
    assert_eq!(moved.labels, [work]);
    assert!(moved.downloaded);
}

#[tokio::test]
async fn mail_moved_mid_pass_into_a_folder_already_read_keeps_its_body() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    let uid = f
        .user
        .append("Work", mail(Some("mv@x"), "Moved", ""))
        .unwrap();
    f.sync().await;
    f.backfill().await;
    let downloads = body_downloads(&f);
    // Once the inbox is read, another client moves the message there.
    let user = f.user.clone();
    f.server.on_next("SELECT", "\"Work\"", move || {
        user.append("INBOX", mail(Some("mv@x"), "Moved", ""))
            .unwrap();
        user.expunge("Work", uid).unwrap();
    });
    assert_eq!(f.sync().await.removed, 0);
    let moved = f
        .message("mv@x")
        .expect("kept: the pass read the inbox first");
    assert!(moved.downloaded);
    assert_eq!(f.sync().await.removed, 0);
    let moved = f.message("mv@x").unwrap();
    assert_eq!(moved.labels, [SystemLabel::Inbox.provider_id()]);
    assert!(moved.downloaded);
    f.backfill().await;
    assert_eq!(body_downloads(&f), downloads, "nothing downloaded again");
}

#[tokio::test]
async fn a_role_moving_to_another_folder_takes_the_label_with_it() {
    let f = fixture().await;
    // Sent by its name only, until the server marks another folder \Sent.
    f.user.create_folder("Sent").unwrap();
    f.user
        .append("Sent", mail(Some("old@x"), "Old", "").flags(&["\\Seen"]))
        .unwrap();
    f.sync().await;
    f.backfill().await;
    assert_eq!(f.labels("old@x"), ["SENT"]);
    f.user
        .create_special_folder("Sent Items", SpecialUse::Sent)
        .unwrap();
    f.user
        .append(
            "Sent Items",
            mail(Some("new@x"), "New", "").flags(&["\\Seen"]),
        )
        .unwrap();
    f.sync().await;
    let sent = f
        .store
        .imap_folder(&f.me, SystemLabel::Sent.provider_id())
        .unwrap()
        .unwrap();
    assert_eq!(sent.name, "Sent Items");
    assert_eq!(f.labels("new@x"), ["SENT"]);
    // The old folder is an ordinary one now, with its mail and body.
    let old = f.label_id("Sent");
    assert_ne!(old, SystemLabel::Sent.provider_id());
    assert_eq!(f.labels("old@x"), std::slice::from_ref(&old));
    assert!(f.message("old@x").unwrap().downloaded);
    f.sync().await;
    f.sync().await;
    assert_eq!(f.labels("old@x"), [old]);
    assert!(f.message("old@x").unwrap().downloaded);
    assert_eq!(f.labels("new@x"), ["SENT"]);
}
