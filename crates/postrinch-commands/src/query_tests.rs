//! Reading the search language: words, operators, dates and sizes.

use super::*;

fn parse(text: &str) -> Query {
    Query::parse(text, Timestamp::from_millis(1_790_000_000_000)).unwrap()
}

#[test]
fn words_and_operators_separate() {
    let q = parse(r#"from:amazon "order shipped" refund* label:Work/Clients is:unread"#);
    assert_eq!(q.words, [r#""order shipped""#, "refund*"]);
    assert_eq!(q.from, ["amazon"]);
    assert_eq!(q.labels, [LabelName::Named("Work/Clients".into())]);
    assert_eq!(q.unread, Some(true));
    assert_eq!(q.text().unwrap(), r#"+"order shipped" +refund*"#);
    assert_eq!(q.terms(), ["order", "shipped", "refund"]);
}

#[test]
fn words_left_out_are_not_required() {
    let q = parse(r#"rota -draft -"weekly report" -the"#);
    assert_eq!(q.text().unwrap(), "+rota");
    // Each as what mail with it matches; a stop word leaves nothing out.
    assert_eq!(q.text_without(), ["+draft", r#"+"weekly report""#]);
    assert_eq!(q.terms(), ["rota"]);
    assert_eq!(parse("-draft").text(), None);
    assert!(parse("-draft").has_words());
}

#[test]
fn quoted_values_and_gmail_text() {
    let q = parse(r#"from:"Ada Lovelace" account:ada@example.org shift"#);
    assert_eq!(q.from, ["Ada Lovelace"]);
    assert_eq!(q.accounts, ["ada@example.org"]);
    assert_eq!(q.gmail, r#"from:"Ada Lovelace" shift"#);
    assert!(q.has_words());
}

#[test]
fn gmails_superset_leaves_out_nothing_but_labels() {
    let q =
        parse(r#"invoice -from:billing -to:"Jo Ann" -cc:x -bcc:y -draft -in:inbox has:attachment"#);
    assert_eq!(
        q.gmail,
        r#"invoice -from:billing -to:"Jo Ann" -cc:x -bcc:y -draft -in:inbox has:attachment"#
    );
    assert_eq!(q.gmail_superset, "invoice -in:inbox has:attachment");
    assert_eq!(parse("-from:jose last:me").gmail_superset, "");
}

#[test]
fn mailboxes_and_set_aside() {
    let q = parse("in:inbox -in:spam category:promotions");
    assert_eq!(
        q.labels,
        [
            LabelName::System(SystemLabel::Inbox),
            LabelName::System(SystemLabel::CategoryPromotions)
        ]
    );
    assert_eq!(q.without, [LabelName::System(SystemLabel::Spam)]);
    assert!(parse("in:trash").set_aside_too);
    assert!(parse("in:anywhere").labels.is_empty());
}

#[test]
fn dates_and_ages() {
    let q = parse("after:2024/01/31 before:2024-02-02");
    let (after, before) = (q.after.unwrap(), q.before.unwrap());
    // Two days apart, whatever the zone (no change of clocks then).
    assert_eq!(before.as_millis() - after.as_millis(), 2 * 86_400_000);
    let q = parse("newer_than:2d");
    let now = 1_790_000_000_000;
    let age = now - q.after.unwrap().as_millis();
    assert!((47 * 3_600_000..=49 * 3_600_000).contains(&age));
}

#[test]
fn sizes_in_bytes() {
    let q = parse("larger:10M smaller:2gb");
    assert_eq!((q.larger, q.smaller), (Some(10 << 20), Some(2 << 30)));
    assert_eq!(parse("size:500").larger, Some(500));
    assert_eq!(parse("larger:200k").larger, Some(200 << 10));
    assert_eq!(parse("larger:3M").gmail, "larger:3M");
    let now = Timestamp::from_millis(0);
    for bad in [
        "larger:big",
        "smaller:10Q",
        "larger:M",
        "larger:99999999999999999999G",
    ] {
        assert!(Query::parse(bad, now).is_err(), "{bad}");
    }
}

#[test]
fn mistakes_say_what_is_wrong() {
    let now = Timestamp::from_millis(0);
    for bad in [
        "is:snoozed",
        "after:yesterday",
        "older_than:3q",
        "\"open",
        "-subject:x",
        "from:",
    ] {
        assert!(Query::parse(bad, now).is_err(), "{bad}");
    }
    let left_out = parse("-from:noreply -cc:list@x.org");
    assert_eq!(
        (left_out.not_from, left_out.not_to),
        (vec!["noreply".to_owned()], vec!["list@x.org".to_owned()])
    );
    // An unknown word with a colon is just a word.
    assert_eq!(parse("re:invoice").words, ["re:invoice"]);
}

#[test]
fn last_is_postrinch_own() {
    let q = parse("last:me in:inbox");
    assert_eq!(q.last, Some(Last::Me));
    assert_eq!(q.gmail, "in:inbox");
    assert_eq!(parse("last:Them").last, Some(Last::Them));
    let now = Timestamp::from_millis(0);
    assert!(Query::parse("last:you", now).is_err());
    assert!(Query::parse("-last:me", now).is_err());
}

#[test]
fn labels_match_as_gmail_writes_them() {
    assert!(label_matches("Work/Clients", "work-clients"));
    assert!(label_matches("Receipts 2024", "receipts-2024"));
    assert!(!label_matches("Work", "work-clients"));
}
