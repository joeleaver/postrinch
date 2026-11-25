//! What can never be read must not hold the rest back for ever: a folder
//! that always refuses SELECT, a message the server lists but never gives
//! headers for. And of copies that arrived at the same moment, the one
//! that gives a message its headers and body is the same whatever order
//! they are read in.

use chrono::{DateTime, FixedOffset};
use postrinch_mailtest::ImapFault;

use super::sync_support::{fixture, mail};

#[tokio::test]
async fn a_folder_that_always_refuses_select_stops_holding_removals_back() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    let a = f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
    f.server.fail_next_matching(
        "SELECT",
        "\"Work\"",
        1000,
        ImapFault::No("Mailbox is broken".into()),
    );
    f.sync().await;
    f.user.expunge("INBOX", a).unwrap();
    for _ in 0..6 {
        f.sync().await;
    }
    assert!(f.message("a@x").is_none(), "a@x is removed in the end");
    assert!(f.message("b@x").is_some());
    assert!(f.label_names().contains(&"Work".to_string()));
}

#[tokio::test]
async fn a_message_that_never_answers_is_kept_as_unreadable() {
    let f = fixture().await;
    let silent = f
        .user
        .append("INBOX", mail(Some("silent@x"), "Silent", ""))
        .unwrap();
    let gone = f
        .user
        .append("INBOX", mail(Some("gone@x"), "Gone", ""))
        .unwrap();
    f.user.withhold("INBOX", silent, true).unwrap();
    f.sync().await;
    f.user.expunge("INBOX", gone).unwrap();
    for _ in 0..4 {
        f.sync().await;
    }
    assert!(f.message("gone@x").is_none(), "removals are not held back");
    assert!(f.message("silent@x").is_none(), "its headers never came");
    let kept: Vec<_> = f
        .store
        .message_ids(&f.me)
        .unwrap()
        .into_iter()
        .filter_map(|id| f.store.message(&f.me, &id).unwrap())
        .collect();
    assert_eq!(kept.len(), 1, "the silent message is kept: {kept:?}");
    assert_eq!(kept[0].headers.message_id, None);
    assert_eq!(kept[0].labels, ["INBOX"]);
    // It is not asked for by its headers again.
    let before = f.server.commands().len();
    f.sync().await;
    let headers = f.server.commands()[before..]
        .iter()
        .filter(|c| c.contains("BODY.PEEK[HEADER]"))
        .count();
    assert_eq!(headers, 0);
}

fn at(text: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(text).unwrap()
}

#[tokio::test]
async fn copies_as_early_choose_by_folder_and_uid_whatever_the_order() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.sync().await;
    // The copy in the folder of the lower label id wins; it is put on
    // the server only after the other was read.
    let (inbox, work) = (f.label_id("INBOX"), f.label_id("Work"));
    let (first, later) = if work < inbox {
        ("INBOX", "Work")
    } else {
        ("Work", "INBOX")
    };
    let when = at("2026-03-04T05:06:07+00:00");
    let copy = |subject: &str| {
        let raw = format!(
            "From: Ann <ann@example.com>\r\nSubject: {subject}\r\nMessage-ID: <m@x>\r\n\
             \r\nThe body of {subject}.\r\n"
        );
        postrinch_mailtest::NewMessage::new(raw.into_bytes()).internal_date(when)
    };
    f.user.append(first, copy("Loser")).unwrap();
    f.sync().await;
    assert_eq!(f.message("m@x").unwrap().subject(), "Loser");
    f.user.append(later, copy("Winner")).unwrap();
    f.sync().await;
    f.backfill().await;
    let m = f.message("m@x").unwrap();
    assert_eq!(m.subject(), "Winner");
    assert!(m.downloaded);
    assert!(
        m.body_text.contains("The body of Winner"),
        "{}",
        m.body_text
    );
    assert_eq!(m.labels.len(), 2);
}
