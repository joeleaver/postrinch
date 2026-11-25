//! The search language as IMAP `SEARCH` keys: every operator, dates,
//! text that is not ASCII, `OR` and leaving out, and the terms IMAP
//! cannot ask.

use jiff::tz::TimeZone;
use postrinch_model::{Day, Label, LabelKind, SearchKey, SystemLabel, Timestamp};

use super::{alternative, key};
use crate::query::Query;
use crate::query_or::Either;

/// 2026-11-25 12:00 UTC: the 25th wherever the tests run, but for zones
/// twelve hours or more from UTC.
const NOW: i64 = 1_795_608_000_000;

fn parse(text: &str) -> Query {
    Query::parse(text, Timestamp::from_millis(NOW)).unwrap()
}

fn keys(text: &str) -> SearchKey {
    // Dates are read, and asked, as days here.
    let (key, inexact) = key(&parse(text), &TimeZone::system());
    assert_eq!(inexact, None, "{text}");
    key
}

fn day(year: i16, month: i8, day: i8) -> Day {
    Day { year, month, day }
}

fn text(s: &str) -> SearchKey {
    SearchKey::Text(s.into())
}

fn any_recipient(s: &str) -> SearchKey {
    SearchKey::either(
        SearchKey::To(s.into()),
        SearchKey::either(SearchKey::Cc(s.into()), SearchKey::Bcc(s.into())),
    )
}

/// An IMAP account's labels: its folders.
fn folders() -> Vec<Label> {
    let system =
        |s: SystemLabel| Label::new(s.provider_id(), s.provider_id(), LabelKind::System(s));
    vec![
        system(SystemLabel::Inbox),
        system(SystemLabel::Sent),
        system(SystemLabel::Trash),
        system(SystemLabel::Spam),
        Label::new("f000000000001", "Work/Clients", LabelKind::User),
        Label::new("f000000000002", "Receipts", LabelKind::User),
    ]
}

#[test]
fn words_and_phrases_are_text() {
    assert_eq!(keys("thursday"), text("thursday"));
    assert_eq!(
        keys(r#""order shipped" refund*"#),
        SearchKey::All(vec![text("order shipped"), text("refund")])
    );
    // Gmail's minus before a word leaves out mail with it.
    assert_eq!(
        keys("rota -draft"),
        SearchKey::All(vec![text("rota"), SearchKey::negated(text("draft"))])
    );
    // Nothing left to ask once the quotes and the star go.
    assert_eq!(keys(r#"x "" *"#), text("x"));
}

#[test]
fn people_and_subject() {
    assert_eq!(
        keys(r#"from:"Ada Lovelace" subject:rota x"#),
        SearchKey::All(vec![
            text("x"),
            SearchKey::From("Ada Lovelace".into()),
            SearchKey::Subject("rota".into()),
        ])
    );
    // to:, cc: and bcc: are any recipient, as the local search reads them.
    for op in ["to", "cc", "bcc"] {
        assert_eq!(
            keys(&format!("{op}:mary x")),
            SearchKey::All(vec![text("x"), any_recipient("mary")])
        );
    }
}

#[test]
fn leaving_out_people_nests_not_over_or() {
    assert_eq!(
        keys("x -from:noreply -to:list@x.org"),
        SearchKey::All(vec![
            text("x"),
            SearchKey::negated(SearchKey::From("noreply".into())),
            SearchKey::negated(any_recipient("list@x.org")),
        ])
    );
}

#[test]
fn flags() {
    let cases = [
        ("is:unread", SearchKey::Unseen),
        ("is:read", SearchKey::Seen),
        ("is:starred", SearchKey::Flagged),
        ("in:starred", SearchKey::Flagged),
        ("is:unstarred", SearchKey::Unflagged),
    ];
    for (op, flag) in cases {
        assert_eq!(
            keys(&format!("x {op}")),
            SearchKey::All(vec![text("x"), flag]),
            "{op}"
        );
    }
}

#[test]
fn dates_go_wider_by_two_days() {
    // after: and before: are days here; the server's day may be another.
    assert_eq!(
        keys("x after:2024/01/31 before:2024/02/02"),
        SearchKey::All(vec![
            text("x"),
            SearchKey::Since(day(2024, 1, 29)),
            SearchKey::Before(day(2024, 2, 5)),
        ])
    );
    // Ages are from now (2026-11-25).
    assert_eq!(
        keys("x newer_than:3d older_than:1y"),
        SearchKey::All(vec![
            text("x"),
            SearchKey::Since(day(2026, 11, 20)),
            SearchKey::Before(day(2025, 11, 28)),
        ])
    );
    // Across a month's and a year's end.
    assert_eq!(
        keys("x after:2026/01/01"),
        SearchKey::All(vec![text("x"), SearchKey::Since(day(2025, 12, 30))])
    );
}

#[test]
fn sizes_are_bytes() {
    assert_eq!(
        keys("x larger:10M smaller:2G"),
        SearchKey::All(vec![
            text("x"),
            SearchKey::Larger(10 << 20),
            SearchKey::Smaller(2 << 30),
        ])
    );
}

#[test]
fn text_that_is_not_ascii_is_kept_as_it_is() {
    // The wire (quoting, literals, CHARSET) is sync's to write.
    assert_eq!(
        keys("from:Zoë Grüße"),
        SearchKey::All(vec![text("Grüße"), SearchKey::From("Zoë".into())])
    );
}

#[test]
fn attachments_cannot_be_asked() {
    let (key, inexact) = key(&parse("invoice has:attachment"), &TimeZone::UTC);
    assert_eq!(key, text("invoice"));
    assert_eq!(inexact.as_deref(), Some("has:attachment"));
}

#[test]
fn what_the_headers_answer_is_left_to_the_check() {
    // last: is a conversation's, Postrinch's own: nothing to ask.
    assert_eq!(keys("x last:them"), text("x"));
    let labels = folders();
    let both = alternative(
        &parse("x label:work-clients label:receipts"),
        &labels,
        &TimeZone::UTC,
    )
    .unwrap();
    // The first label is where to look; the second is checked after.
    assert_eq!(both.folders, ["f000000000001"]);
    assert_eq!(both.key, text("x"));
}

#[test]
fn labels_are_folders() {
    let labels = folders();
    let one = |q: &str| alternative(&parse(q), &labels, &TimeZone::UTC);
    let inbox = one("x in:inbox").unwrap();
    assert_eq!(
        (inbox.folders, inbox.set_aside_too),
        (vec!["INBOX".into()], false)
    );
    let trash = one("x in:trash").unwrap();
    assert_eq!(
        (trash.folders, trash.set_aside_too),
        (vec!["TRASH".into()], true)
    );
    let everywhere = one("x in:anywhere").unwrap();
    assert!(everywhere.folders.is_empty() && everywhere.set_aside_too);
    let not = one("x -label:receipts -in:sent").unwrap();
    assert!(not.folders.is_empty());
    assert_eq!(not.not_in, ["f000000000002", "SENT"]);
    // An account without categories has its whole inbox as Primary.
    assert_eq!(one("x category:primary").unwrap().folders, ["INBOX"]);
    // Labels it does not have match nothing in it.
    assert!(one("x category:social").is_none());
    assert!(one("x is:important").is_none());
    assert!(one("x label:elsewhere").is_none());
    // Gmail's own words go with it, for Gmail's servers.
    assert_eq!(inbox.gmail, "x in:inbox");
}

#[test]
fn or_spreads_into_alternatives() {
    let now = Timestamp::from_millis(NOW);
    let either = Either::parse("(from:ann OR from:bob) rota", now).unwrap();
    let labels = folders();
    let keys: Vec<SearchKey> = either
        .queries
        .iter()
        .map(|q| alternative(q, &labels, &TimeZone::UTC).unwrap().key)
        .collect();
    assert_eq!(
        keys,
        [
            SearchKey::All(vec![text("rota"), SearchKey::From("ann".into())]),
            SearchKey::All(vec![text("rota"), SearchKey::From("bob".into())]),
        ]
    );
}
