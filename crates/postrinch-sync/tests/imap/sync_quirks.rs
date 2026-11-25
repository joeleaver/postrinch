//! Servers that answer in ways the sync must take in its stride: FETCH
//! answers split in two or mixed with updates nobody asked for, mail that
//! cannot be read, folder names that are not 7-bit, copies of one message
//! with different INTERNALDATEs, and folders too big to list in one go.

use chrono::{DateTime, FixedOffset};
use postrinch_mailtest::{Extension, ImapConfig, NewMessage};
use postrinch_model::{FolderRoles, SystemLabel};

use super::sync_support::{Fixture, fixture, fixture_with, fixture_without, mail};

/// Two inbox messages, one read, synced and downloaded; then a third
/// arrives and the read one is marked unread elsewhere.
async fn flags_and_new_mail_come_through(f: &Fixture) {
    let read = f
        .user
        .append("INBOX", mail(Some("a@x"), "A", "").flags(&["\\Seen"]))
        .unwrap();
    f.user
        .append("INBOX", mail(Some("b@x"), "B", "").flags(&["\\Flagged"]))
        .unwrap();
    f.sync().await;
    f.backfill().await;
    assert!(!f.message("a@x").unwrap().flags.unread);
    assert!(f.message("b@x").unwrap().flags.starred);
    assert!(f.message("b@x").unwrap().downloaded);
    f.user.remove_flags("INBOX", read, &["\\Seen"]).unwrap();
    f.user.append("INBOX", mail(Some("c@x"), "C", "")).unwrap();
    f.sync().await;
    f.backfill().await;
    assert!(f.message("a@x").unwrap().flags.unread);
    let b = f.message("b@x").unwrap();
    assert!(b.flags.starred && b.flags.unread);
    for id in ["a@x", "b@x", "c@x"] {
        assert!(f.message(id).unwrap().downloaded, "{id} downloaded");
    }
    assert_eq!(f.message_count(), 3);
}

#[tokio::test]
async fn unsolicited_flag_updates_change_nothing_they_should_not() {
    let f = fixture_with(
        ImapConfig::new().unsolicited_fetches(true),
        FolderRoles::default(),
    )
    .await;
    flags_and_new_mail_come_through(&f).await;
}

#[tokio::test]
async fn answers_split_in_two_are_merged() {
    let f = fixture_with(
        ImapConfig::new().splits_fetches(true),
        FolderRoles::default(),
    )
    .await;
    flags_and_new_mail_come_through(&f).await;
}

#[tokio::test]
async fn answers_split_in_two_are_merged_without_condstore() {
    let config = ImapConfig::new()
        .without(Extension::Condstore)
        .without(Extension::Qresync)
        .splits_fetches(true)
        .unsolicited_fetches(true);
    let f = fixture_with(config, FolderRoles::default()).await;
    flags_and_new_mail_come_through(&f).await;
}

#[tokio::test]
async fn a_message_that_cannot_be_read_does_not_stop_the_sync() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    let empty = f.user.append("INBOX", NewMessage::new(Vec::new())).unwrap();
    f.user
        .append("INBOX", mail(Some("ok@x"), "Fine", ""))
        .unwrap();
    f.user
        .append("Work", mail(Some("w@x"), "Work", ""))
        .unwrap();
    f.sync().await;
    assert_eq!(f.message_count(), 3, "the unreadable one is kept too");
    assert_eq!(f.labels("w@x"), [f.label_id("Work")]);
    f.backfill().await;
    assert!(f.message("ok@x").unwrap().downloaded);
    assert!(f.message("w@x").unwrap().downloaded);
    let ids = f.store.message_ids(&f.me).unwrap();
    let odd: Vec<_> = ids
        .iter()
        .filter_map(|id| f.store.message(&f.me, id).unwrap())
        .filter(|m| m.headers.message_id.is_none())
        .collect();
    assert_eq!(odd.len(), 1);
    assert!(
        odd[0].id.0.starts_with('u'),
        "named by its UID: {:?}",
        odd[0].id
    );
    assert_eq!(odd[0].labels, [SystemLabel::Inbox.provider_id()]);
    assert!(odd[0].downloaded, "backfill does not ask for it again");
    let asked = |f: &Fixture| {
        f.server
            .commands()
            .iter()
            .filter(|c| c.contains("BODY.PEEK[]") && c.contains(&empty.to_string()))
            .count()
    };
    let before = asked(&f);
    f.backfill().await;
    f.sync().await;
    f.backfill().await;
    assert_eq!(asked(&f), before);
    assert_eq!(f.message_count(), 3);
}

/// A folder named with 8-bit characters, as some servers list them,
/// is opened by a literal: non-synchronizing with LITERAL+, waited for
/// without.
async fn an_eight_bit_folder_name_is_synced(literal_plus: bool) {
    let config = ImapConfig::new().literal_plus(literal_plus);
    let f = fixture_with(config, FolderRoles::default()).await;
    f.user.create_folder("Entwürfe").unwrap();
    f.user
        .append("Entwürfe", mail(Some("e@x"), "Entwurf", ""))
        .unwrap();
    f.sync().await;
    f.backfill().await;
    let label = f.label_id("Entwürfe");
    assert_eq!(f.labels("e@x"), [label]);
    assert!(f.message("e@x").unwrap().downloaded);
    // The fake logs a literal as `{n}`: "Entwürfe" is nine bytes.
    let commands = f.server.commands();
    assert!(
        commands.iter().any(|c| c.starts_with("SELECT {9}")),
        "{commands:?}"
    );
}

#[tokio::test]
async fn an_eight_bit_folder_name_is_sent_as_a_literal_plus() {
    an_eight_bit_folder_name_is_synced(true).await;
}

#[tokio::test]
async fn an_eight_bit_folder_name_is_sent_as_a_synchronizing_literal() {
    an_eight_bit_folder_name_is_synced(false).await;
}

#[tokio::test]
async fn a_folder_name_that_cannot_be_sent_back_is_skipped() {
    let f = fixture().await;
    // What imap-proto makes of a name that is not UTF-8.
    f.user.create_folder("Bad\u{FFFD}").unwrap();
    f.user
        .append("Bad\u{FFFD}", mail(Some("bad@x"), "Bad", ""))
        .unwrap();
    f.user
        .append("INBOX", mail(Some("in@x"), "In", ""))
        .unwrap();
    f.sync().await;
    assert!(f.message("bad@x").is_none());
    assert_eq!(f.labels("in@x"), [SystemLabel::Inbox.provider_id()]);
    assert!(!f.label_names().iter().any(|n| n.starts_with("Bad")));
}

fn at(text: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(text).unwrap()
}

#[tokio::test]
async fn the_copy_with_the_earliest_internaldate_is_kept() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    let (early, late) = (
        at("2026-01-02T03:04:05+00:00"),
        at("2026-06-07T08:09:10+00:00"),
    );
    // The inbox is read first: the early copy of a@x comes first, of b@x
    // last.
    for (folder, id, date) in [
        ("INBOX", "a@x", early),
        ("Work", "a@x", late),
        ("INBOX", "b@x", late),
        ("Work", "b@x", early),
    ] {
        f.user
            .append(folder, mail(Some(id), id, "").internal_date(date))
            .unwrap();
    }
    f.sync().await;
    let millis = early.timestamp_millis();
    for id in ["a@x", "b@x"] {
        let m = f.message(id).unwrap();
        assert_eq!(m.received.as_millis(), millis, "{id}");
        assert_eq!(m.labels.len(), 2, "{id} is in both folders");
    }
    f.sync().await;
    assert_eq!(f.message("b@x").unwrap().received.as_millis(), millis);
}

#[tokio::test]
async fn a_big_folder_without_condstore_is_listed_in_chunks() {
    let f = fixture_without(&[Extension::Condstore, Extension::Qresync]).await;
    // One more than a batch of 1000, so the listing takes two.
    for n in 0..1001 {
        let id = format!("n{n}@x");
        f.user.append("INBOX", mail(Some(&id), "Many", "")).unwrap();
    }
    let listings = |f: &Fixture| {
        f.server
            .commands()
            .iter()
            .filter(|c| c.contains("(UID FLAGS)"))
            .cloned()
            .collect::<Vec<_>>()
    };
    f.sync().await;
    assert_eq!(f.message_count(), 1001);
    let first = listings(&f);
    assert_eq!(first.len(), 2, "1001 UIDs in batches of 1000: {first:?}");
    f.sync().await;
    let both = listings(&f);
    assert_eq!(both.len(), 4, "{both:?}");
    assert!(both.iter().all(|c| !c.contains(":*")), "{both:?}");
    assert_eq!(f.message_count(), 1001);
}
