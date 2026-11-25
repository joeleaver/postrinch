//! Servers that will not answer for everything: a FETCH that ends `NO`
//! for some messages, a server too busy to answer, and what that must
//! not cost (a folder's changes skipped, the other folders unread).

use std::sync::Arc;

use postrinch_mailtest::ImapFault;
use postrinch_sync::SyncError;

use super::sync_support::{fixture, mail};

#[tokio::test]
async fn a_message_without_headers_does_not_hide_later_expunges() {
    // QRESYNC: a message that went unanswered must not leave a window of
    // UIDs that are never checked for removal again.
    let f = fixture().await;
    for id in ["a@x", "b@x", "c@x"] {
        f.user.append("INBOX", mail(Some(id), id, "")).unwrap();
    }
    f.sync().await;
    let d = f.user.append("INBOX", mail(Some("d@x"), "D", "")).unwrap();
    let e = f.user.append("INBOX", mail(Some("e@x"), "E", "")).unwrap();
    f.user.append("INBOX", mail(Some("f@x"), "F", "")).unwrap();
    f.user.withhold("INBOX", d, true).unwrap();
    f.sync().await;
    assert!(f.message("d@x").is_none(), "d@x has not come yet");
    assert!(f.message("e@x").is_some() && f.message("f@x").is_some());
    // e@x goes while d@x is still owed; d@x comes back.
    f.user.expunge("INBOX", e).unwrap();
    f.user.withhold("INBOX", d, false).unwrap();
    f.sync().await;
    assert!(f.message("d@x").is_some(), "d@x is asked for again");
    assert!(f.labels("e@x").is_empty(), "e@x left the inbox");
    f.sync().await;
    assert!(f.message("e@x").is_none(), "e@x is removed");
    assert_eq!(f.message_count(), 5);
}

#[tokio::test]
async fn a_fetch_ending_no_keeps_what_came_and_the_other_folders() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    let bad = f
        .user
        .append("INBOX", mail(Some("bad@x"), "Bad", ""))
        .unwrap();
    f.user
        .append("INBOX", mail(Some("ok@x"), "Fine", ""))
        .unwrap();
    f.user
        .append("Work", mail(Some("w@x"), "Work", ""))
        .unwrap();
    f.user.withhold("INBOX", bad, true).unwrap();
    let report = f.sync().await;
    assert_eq!(report.headers, 2);
    assert!(f.message("ok@x").is_some() && f.message("w@x").is_some());
    assert!(f.message("bad@x").is_none());
    // Asked for again, and stored once it answers.
    f.user.withhold("INBOX", bad, false).unwrap();
    f.sync().await;
    assert_eq!(f.labels("bad@x"), ["INBOX"]);
}

#[tokio::test]
async fn a_busy_server_stops_the_pass_and_loses_nothing() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.server.fail_next(
        "UID FETCH",
        1,
        ImapFault::No("[UNAVAILABLE] Temporary System Error".into()),
    );
    let result = f.sync.sync(&f.me, Arc::new(|_| {})).await;
    assert!(
        matches!(result, Err(SyncError::Busy { .. })),
        "busy, not a partial answer: {result:?}"
    );
    f.sync().await;
    assert!(f.message("a@x").is_some());
}
