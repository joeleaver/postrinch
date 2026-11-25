//! Servers that will not answer for everything: a FETCH that ends `NO`
//! for some messages, a pass cut short, and what that must not cost (a
//! folder's changes skipped, the other folders unread, the same headers
//! downloaded pass after pass).

use std::sync::Arc;

use postrinch_mailtest::ImapFault;
use postrinch_sync::SyncError;

use super::sync_support::{fixture, header_fetches, mail};

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
async fn mail_stored_by_a_pass_that_did_not_move_is_not_fetched_again() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("x@x"), "X", "")).unwrap();
    f.sync().await;
    let burst: Vec<u32> = ["b@x", "c@x", "d@x", "e@x", "g@x"]
        .iter()
        .map(|id| f.user.append("INBOX", mail(Some(id), id, "")).unwrap())
        .collect();
    let (b, c, d) = (burst[0], burst[1], burst[2]);
    f.user.withhold("INBOX", d, true).unwrap();
    // The flag changes cannot be read: the folder's position stays.
    f.server.fail_next_matching(
        "UID FETCH",
        "CHANGEDSINCE",
        1,
        ImapFault::No("[SERVERBUG] Internal error".into()),
    );
    f.sync().await;
    assert_eq!(f.message_count(), 5, "all but d@x");
    f.user.withhold("INBOX", d, false).unwrap();
    f.user.add_flags("INBOX", b, &["\\Seen"]).unwrap();
    f.user.expunge("INBOX", c).unwrap();
    let before = f.server.commands().len();
    f.sync().await;
    let asked = header_fetches(&f.server.commands()[before..]);
    assert_eq!(asked, [d], "only the one not yet stored");
    assert!(f.message("d@x").is_some());
    assert!(
        !f.message("b@x").unwrap().flags.unread,
        "flags still follow"
    );
}

#[tokio::test]
async fn a_burst_cut_short_by_the_bandwidth_limit_goes_on_where_it_stopped() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("x@x"), "X", "")).unwrap();
    f.sync().await;
    for n in 0..150 {
        let id = format!("{n}@burst");
        f.user.append("INBOX", mail(Some(&id), &id, "")).unwrap();
    }
    // Headers come 100 at a time, newest first: UIDs 52 to 151, then 2
    // to 51, which Gmail turns away once the day's bandwidth is spent.
    f.server.fail_next_matching(
        "UID FETCH",
        "FETCH 2:51 (UID FLAGS",
        1,
        ImapFault::No("[ALERT] Account exceeded command or bandwidth limits. (Failure)".into()),
    );
    let result = f.sync.sync(&f.me, Arc::new(|_| {})).await;
    assert!(matches!(result, Err(SyncError::Busy { .. })), "{result:?}");
    assert_eq!(f.message_count(), 101);
    let before = f.server.commands().len();
    f.sync().await;
    let asked = header_fetches(&f.server.commands()[before..]);
    assert_eq!(asked, (2..=51).collect::<Vec<u32>>());
    assert_eq!(f.message_count(), 151);
}
