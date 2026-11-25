//! IMAP sync against the fake server: first sync, backfill, new mail,
//! flags, gone mail, UIDVALIDITY, and downloading on demand.

use postrinch_mailtest::Extension;
use postrinch_model::{AccountState, ImapFolder};
use postrinch_sync::SyncError;

use super::sync_support::{fixture, fixture_without, mail};

#[tokio::test]
async fn first_sync_stores_headers_then_backfill_downloads_bodies() {
    let f = fixture().await;
    for n in 1..=3 {
        f.user
            .append(
                "INBOX",
                mail(Some(&format!("m{n}@x")), &format!("Mail {n}"), ""),
            )
            .unwrap();
    }
    let report = f.sync().await;
    assert!(report.full);
    assert_eq!(report.headers, 3);
    let one = f.message("m1@x").unwrap();
    assert!(!one.downloaded);
    assert_eq!(one.subject(), "Mail 1");
    assert_eq!(one.labels, ["INBOX"]);
    assert!(one.flags.unread);
    assert_eq!(f.store.undownloaded(&f.me).unwrap().len(), 3);

    let report = f.backfill().await;
    assert_eq!(report.fetched, 3);
    let one = f.message("m1@x").unwrap();
    assert!(one.downloaded);
    assert!(one.body_text.contains("The body of Mail 1"));
    assert!(f.store.undownloaded(&f.me).unwrap().is_empty());
    // Downloading does not read the mail.
    assert!(!f.user.message("INBOX", 1).unwrap().has_flag("\\Seen"));
    assert!(f.message("m1@x").unwrap().flags.unread);
}

#[tokio::test]
async fn a_second_sync_fetches_only_new_mail() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.sync().await;
    f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    let report = f.sync().await;
    assert!(!report.full);
    assert_eq!(report.headers, 1);
    assert!(f.message("b@x").is_some());
    let nothing = f.sync().await;
    assert_eq!(
        (nothing.headers, nothing.relabelled, nothing.removed),
        (0, 0, 0)
    );
}

async fn flags_follow_the_server(without: &[Extension]) {
    let f = fixture_without(without).await;
    let a = f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    let b = f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    f.sync().await;
    f.user.add_flags("INBOX", a, &["\\Seen"]).unwrap();
    f.user.add_flags("INBOX", b, &["\\Flagged"]).unwrap();
    let report = f.sync().await;
    assert_eq!(report.relabelled, 2);
    assert!(!f.message("a@x").unwrap().flags.unread);
    let b_flags = f.message("b@x").unwrap().flags;
    assert!(b_flags.starred && b_flags.unread);
    f.user.remove_flags("INBOX", a, &["\\Seen"]).unwrap();
    f.sync().await;
    assert!(f.message("a@x").unwrap().flags.unread);
}

#[tokio::test]
async fn flag_changes_by_condstore() {
    flags_follow_the_server(&[]).await;
}

#[tokio::test]
async fn flag_changes_without_condstore() {
    flags_follow_the_server(&[Extension::Condstore, Extension::Qresync]).await;
}

#[tokio::test]
async fn gone_mail_by_qresync_vanished() {
    let f = fixture().await;
    let a = f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    f.sync().await;
    let searches = f.server.command_count("UID SEARCH");
    f.user.expunge("INBOX", a).unwrap();
    let report = f.sync().await;
    // It waits for a complete pass that started after it went.
    assert_eq!(report.removed, 0);
    assert!(f.message("a@x").unwrap().labels.is_empty());
    assert_eq!(f.sync().await.removed, 1);
    assert!(f.message("a@x").is_none());
    assert!(f.message("b@x").is_some());
    let commands = f.server.commands();
    assert!(commands.iter().any(|c| c.contains("VANISHED")));
    assert_eq!(f.server.command_count("UID SEARCH"), searches);
}

async fn gone_mail_by_comparing(without: &[Extension]) {
    let f = fixture_without(without).await;
    let a = f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    f.sync().await;
    let searches = f.server.command_count("UID SEARCH");
    f.user.expunge("INBOX", a).unwrap();
    assert_eq!(f.sync().await.removed, 0);
    assert_eq!(f.sync().await.removed, 1);
    assert!(f.message("a@x").is_none());
    assert!(f.message("b@x").is_some());
    // Without VANISHED, the folder's UIDs are listed to compare.
    assert!(f.server.command_count("UID SEARCH") > searches);
}

#[tokio::test]
async fn gone_mail_with_condstore_but_no_qresync() {
    gone_mail_by_comparing(&[Extension::Qresync]).await;
}

#[tokio::test]
async fn gone_mail_without_condstore() {
    gone_mail_by_comparing(&[Extension::Qresync, Extension::Condstore]).await;
}

#[tokio::test]
async fn the_last_copy_emptied_out_of_a_folder() {
    let f = fixture().await;
    let a = f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.sync().await;
    f.user.expunge("INBOX", a).unwrap();
    f.sync().await;
    f.sync().await;
    assert!(f.message("a@x").is_none());
    assert_eq!(f.message_count(), 0);
}

#[tokio::test]
async fn a_new_uidvalidity_reads_the_folder_again_and_keeps_bodies() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    let gone = f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    f.sync().await;
    f.backfill().await;
    f.user.expunge("INBOX", gone).unwrap();
    f.user.set_uid_validity("INBOX", 4242).unwrap();
    let report = f.sync().await;
    assert!(report.full);
    f.sync().await;
    let a = f.message("a@x").unwrap();
    assert!(a.downloaded, "a message still there keeps its body");
    assert!(f.message("b@x").is_none());
    let places = f.store.imap_message_places(&f.me, &a.id).unwrap();
    assert_eq!(places.len(), 1);
    assert_eq!(places[0].uidvalidity, 4242);
}

#[tokio::test]
async fn a_folder_kept_with_uidvalidity_zero_is_read_whole() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.sync().await;
    let inbox = f.store.imap_folder(&f.me, "INBOX").unwrap().unwrap();
    f.store
        .put_imap_folder(
            &f.me,
            &ImapFolder {
                uidvalidity: 0,
                uidnext: 0,
                highest_modseq: 0,
                ..inbox
            },
        )
        .unwrap();
    let report = f.sync().await;
    assert!(report.full);
    let a = f.message("a@x").unwrap();
    assert_eq!(a.labels, ["INBOX"]);
    let places = f.store.imap_message_places(&f.me, &a.id).unwrap();
    assert_eq!(places.len(), 1);
    assert_ne!(places[0].uidvalidity, 0);
}

#[tokio::test]
async fn a_copy_marked_deleted_counts_as_gone() {
    let f = fixture().await;
    let a = f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.sync().await;
    f.user.add_flags("INBOX", a, &["\\Deleted"]).unwrap();
    assert_eq!(f.sync().await.removed, 0);
    assert!(f.message("a@x").unwrap().labels.is_empty());
    assert_eq!(f.sync().await.removed, 1);
    assert!(f.message("a@x").is_none());
    f.user.remove_flags("INBOX", a, &["\\Deleted"]).unwrap();
    f.sync().await;
    assert_eq!(f.message("a@x").unwrap().labels, ["INBOX"]);
}

#[tokio::test]
async fn download_now_fetches_one_message() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    f.sync().await;
    let got = f.sync.download_now(&f.me, vec![f.id("b@x")]).await.unwrap();
    assert_eq!(got, 1);
    assert!(f.message("b@x").unwrap().downloaded);
    assert!(!f.message("a@x").unwrap().downloaded);
}

#[tokio::test]
async fn connections_are_kept_and_reopened_when_dropped() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.sync().await;
    f.sync().await;
    f.backfill().await;
    let sign_ins = || f.server.command_count("LOGIN") + f.server.command_count("AUTHENTICATE");
    assert_eq!(sign_ins(), 1);
    f.server.disconnect_all();
    f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    assert_eq!(f.sync().await.headers, 1);
    assert_eq!(sign_ins(), 2);
}

#[tokio::test]
async fn a_refused_password_marks_the_account() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    let secrets = postrinch_sync::imap::ImapSecrets::new(std::sync::Arc::new(
        postrinch_sync::MemorySecrets::default(),
    ));
    secrets
        .set(&f.me, &postrinch_sync::imap::ImapSecret::password("wrong"))
        .await
        .unwrap();
    let sync = postrinch_sync::imap::ImapSync::new(
        f.store.clone(),
        super::support::connector(f.server.certificate()),
        secrets,
        std::sync::Arc::new(super::support::Tokens::none()),
    );
    let result = sync.sync(&f.me, std::sync::Arc::new(|_| {})).await;
    assert!(
        matches!(result, Err(SyncError::NeedsSignIn(_))),
        "{result:?}"
    );
    let state = f.store.account(&f.me).unwrap().unwrap().state;
    assert!(matches!(state, AccountState::NeedsSignIn(_)));
}

#[tokio::test]
async fn the_calls_can_run_on_any_thread() {
    fn send<T: Send>(_: &T) {}
    let f = fixture().await;
    let quiet: postrinch_sync::google::ProgressFn = std::sync::Arc::new(|_| {});
    send(&f.sync.sync(&f.me, quiet.clone()));
    send(&f.sync.backfill(&f.me, quiet));
    send(&f.sync.download_now(&f.me, Vec::new()));
    send(&f.sync.disconnect(&f.me));
}
