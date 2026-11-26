//! The search language as IMAP `SEARCH` keys: every operator, dates,
//! text that is not ASCII, `OR`, what is left out (never asked), and the
//! terms IMAP cannot ask.

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
    // A phrase is its terms, wherever they are; a prefix its prefix.
    assert_eq!(
        keys(r#""order shipped" refund*"#),
        SearchKey::All(vec![text("order"), text("ship"), text("refund")])
    );
    // Nothing left to ask once the quotes and the star go.
    assert_eq!(keys(r#"rota "" *"#), text("rota"));
}

#[test]
fn words_are_asked_as_search_here_reads_them() {
    // Two terms here, so two keys: `invoice 4471` is found too.
    assert_eq!(
        keys("invoice-4471"),
        SearchKey::All(vec![text("invoic"), text("4471")])
    );
    // By the stem: `shipping` finds what `shipped` does here.
    assert_eq!(keys("shipping"), text("ship"));
    assert_eq!(keys("happiness"), text("happ"));
    // A word the stemmer lists is asked as itself too.
    assert_eq!(keys("dying"), SearchKey::either(text("di"), text("dying")));
    // Stop words are no terms here, so nothing to ask.
    // `state` is `state` here, and so is `stated`: the `e` is not asked.
    assert_eq!(
        keys(r#""state of the art""#),
        SearchKey::All(vec![text("stat"), text("art")])
    );
    assert!(keys("the").is_every());
}

#[test]
fn words_left_out_are_left_to_the_check() {
    // NOT TEXT would leave out more than search here does: `-cat` and
    // "concatenate", or a word in a header search here does not read.
    assert_eq!(keys("rota -draft"), text("rota"));
    assert!(keys("-draft").is_every());
}

#[test]
fn people_and_subject() {
    assert_eq!(
        keys(r#"from:"Ada Lovelace" subject:rota rota"#),
        SearchKey::All(vec![
            text("rota"),
            SearchKey::From("Ada Lovelace".into()),
            SearchKey::Subject("rota".into()),
        ])
    );
    // to:, cc: and bcc: are any recipient, as the local search reads them.
    for op in ["to", "cc", "bcc"] {
        assert_eq!(
            keys(&format!("{op}:mary rota")),
            SearchKey::All(vec![text("rota"), any_recipient("mary")])
        );
    }
}

#[test]
fn people_left_out_are_left_to_the_check() {
    // NOT FROM would leave out what the server matches, its own way (a
    // server that folds accents leaves out "José" for `-from:jose`): no
    // one left out is asked, whatever the operator.
    for op in ["from", "to", "cc", "bcc"] {
        assert_eq!(keys(&format!("rota -{op}:jose")), text("rota"), "-{op}:");
        assert!(keys(&format!("-{op}:jose")).is_every(), "-{op}:");
    }
    assert_eq!(
        keys("rota from:ada -from:noreply -to:list@x.org"),
        SearchKey::All(vec![text("rota"), SearchKey::From("ada".into())])
    );
    assert_eq!(
        keys("to:mary -cc:list@x.org -bcc:jose"),
        any_recipient("mary")
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
            keys(&format!("rota {op}")),
            SearchKey::All(vec![text("rota"), flag]),
            "{op}"
        );
    }
}

#[test]
fn dates_go_wider_by_two_days() {
    // after: and before: are days here; the server's day may be another.
    assert_eq!(
        keys("rota after:2024/01/31 before:2024/02/02"),
        SearchKey::All(vec![
            text("rota"),
            SearchKey::Since(day(2024, 1, 29)),
            SearchKey::Before(day(2024, 2, 5)),
        ])
    );
    // Ages are from now (2026-11-25).
    assert_eq!(
        keys("rota newer_than:3d older_than:1y"),
        SearchKey::All(vec![
            text("rota"),
            SearchKey::Since(day(2026, 11, 20)),
            SearchKey::Before(day(2025, 11, 28)),
        ])
    );
    // Across a month's and a year's end.
    assert_eq!(
        keys("rota after:2026/01/01"),
        SearchKey::All(vec![text("rota"), SearchKey::Since(day(2025, 12, 30))])
    );
}

#[test]
fn sizes_are_bytes() {
    assert_eq!(
        keys("rota larger:10M smaller:2G"),
        SearchKey::All(vec![
            text("rota"),
            SearchKey::Larger(10 << 20),
            SearchKey::Smaller(2 << 30),
        ])
    );
}

#[test]
fn text_that_is_not_ascii_keeps_to_the_letters_typed() {
    // Search here folds diacritics (`grusse`'s stem for `Grüße`); what is
    // asked is in mail written as typed. The wire (quoting, literals,
    // CHARSET) is sync's to write.
    assert_eq!(
        keys("from:Zoë Grüße"),
        SearchKey::All(vec![text("gr"), SearchKey::From("Zoë".into())])
    );
    assert_eq!(keys("привет"), text("привет"));
}

#[test]
fn attachments_cannot_be_asked() {
    let (key, inexact) = key(&parse("invoice has:attachment"), &TimeZone::UTC);
    assert_eq!(key, text("invoic"));
    assert_eq!(inexact.as_deref(), Some("has:attachment"));
}

#[test]
fn what_the_headers_answer_is_left_to_the_check() {
    // last: is a conversation's, Postrinch's own: nothing to ask.
    assert_eq!(keys("rota last:them"), text("rota"));
    let labels = folders();
    let both = alternative(
        &parse("rota label:work-clients label:receipts"),
        &labels,
        &TimeZone::UTC,
    )
    .unwrap();
    // The first label is where to look; the second is checked after.
    assert_eq!(both.folders, ["f000000000001"]);
    assert_eq!(both.key, text("rota"));
}

#[test]
fn labels_are_folders() {
    let labels = folders();
    let one = |q: &str| alternative(&parse(q), &labels, &TimeZone::UTC);
    let inbox = one("rota in:inbox").unwrap();
    assert_eq!(
        (inbox.folders, inbox.set_aside_too),
        (vec!["INBOX".into()], false)
    );
    let trash = one("rota in:trash").unwrap();
    assert_eq!(
        (trash.folders, trash.set_aside_too),
        (vec!["TRASH".into()], true)
    );
    let everywhere = one("rota in:anywhere").unwrap();
    assert!(everywhere.folders.is_empty() && everywhere.set_aside_too);
    let not = one("rota -label:receipts -in:sent").unwrap();
    assert!(not.folders.is_empty());
    assert_eq!(not.not_in, ["f000000000002", "SENT"]);
    // An account without categories has its whole inbox as Primary.
    assert_eq!(one("rota category:primary").unwrap().folders, ["INBOX"]);
    // Labels it does not have match nothing in it.
    assert!(one("rota category:social").is_none());
    assert!(one("rota is:important").is_none());
    assert!(one("rota label:elsewhere").is_none());
    // Gmail's own words go with it, for Gmail's servers.
    assert_eq!(inbox.gmail, "rota in:inbox");
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
