//! A server search's keys ([`SearchKey`]) as a `UID SEARCH` command
//! (RFC 3501 6.4.4), and Gmail's own language as one (`X-GM-RAW`).
//!
//! Strings go as quoted strings when they are printable ASCII (with `\`
//! and `"` escaped), and as literals otherwise, as RFC 3501 allows for
//! every string argument. A command with any string that is not ASCII
//! says `CHARSET UTF-8` first; one without says no charset, which every
//! server takes (US-ASCII is the default). A server that cannot search
//! in UTF-8 refuses with `[BADCHARSET]`, which the search reports.

use postrinch_model::{Day, SearchKey};

use super::names::Command;

/// `UID SEARCH` for these keys.
pub(super) fn uid_search(key: &SearchKey) -> Command {
    write(key, start(ascii(key)))
}

/// `UID SEARCH X-GM-RAW` for a search in Gmail's language, on Gmail's
/// servers (X-GM-EXT-1).
pub(super) fn gmail_raw(text: &str) -> Command {
    string(start(text.is_ascii()).then("X-GM-RAW "), text)
}

fn start(ascii: bool) -> Command {
    let command = Command::plain("UID SEARCH ");
    if ascii {
        command
    } else {
        command.then("CHARSET UTF-8 ")
    }
}

/// Whether every string of the keys is ASCII.
fn ascii(key: &SearchKey) -> bool {
    match key {
        SearchKey::Text(s)
        | SearchKey::From(s)
        | SearchKey::To(s)
        | SearchKey::Cc(s)
        | SearchKey::Bcc(s)
        | SearchKey::Subject(s) => s.is_ascii(),
        SearchKey::Not(k) => ascii(k),
        SearchKey::Or(a, b) => ascii(a) && ascii(b),
        SearchKey::All(keys) => keys.iter().all(ascii),
        _ => true,
    }
}

fn write(key: &SearchKey, command: Command) -> Command {
    match key {
        SearchKey::Text(s) => string(command.then("TEXT "), s),
        SearchKey::From(s) => string(command.then("FROM "), s),
        SearchKey::To(s) => string(command.then("TO "), s),
        SearchKey::Cc(s) => string(command.then("CC "), s),
        SearchKey::Bcc(s) => string(command.then("BCC "), s),
        SearchKey::Subject(s) => string(command.then("SUBJECT "), s),
        SearchKey::Seen => command.then("SEEN"),
        SearchKey::Unseen => command.then("UNSEEN"),
        SearchKey::Flagged => command.then("FLAGGED"),
        SearchKey::Unflagged => command.then("UNFLAGGED"),
        SearchKey::Since(day) => command.then(&format!("SINCE {}", date(*day))),
        SearchKey::Before(day) => command.then(&format!("BEFORE {}", date(*day))),
        // IMAP's numbers are 32 bits: a size past them is every message's
        // bound.
        SearchKey::Larger(n) => command.then(&format!("LARGER {}", number(*n))),
        SearchKey::Smaller(n) => command.then(&format!("SMALLER {}", number(*n))),
        SearchKey::Not(k) => write(k, command.then("NOT ")),
        SearchKey::Or(a, b) => write(b, write(a, command.then("OR ")).then(" ")),
        SearchKey::All(keys) => match keys.as_slice() {
            [] => command.then("ALL"),
            [one] => write(one, command),
            many => {
                let mut command = command.then("(");
                for (i, k) in many.iter().enumerate() {
                    if i > 0 {
                        command = command.then(" ");
                    }
                    command = write(k, command);
                }
                command.then(")")
            }
        },
    }
}

/// A string argument: quoted when it can be, a literal otherwise. No
/// string may hold a NUL, so none is sent.
fn string(command: Command, text: &str) -> Command {
    let text = text.replace('\0', "");
    let quotable = text.bytes().all(|b| (0x20..0x7f).contains(&b));
    if quotable {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        command.then(&format!("\"{escaped}\""))
    } else {
        command.literal(text.into_bytes())
    }
}

fn number(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// IMAP's date: `1-Feb-1994`.
fn date(day: Day) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = usize::try_from(day.month - 1)
        .ok()
        .and_then(|m| MONTHS.get(m))
        .copied()
        .unwrap_or("Jan");
    format!("{}-{month}-{}", day.day, day.year)
}

#[cfg(test)]
mod tests {
    use super::super::names::Line;
    use super::*;

    /// The command's first line (with LITERAL+) and the lines after.
    fn wire(command: &Command) -> (String, Vec<Vec<u8>>) {
        let wire = command.wire(true, false);
        (wire.first, wire.rest.into_iter().map(|l| l.bytes).collect())
    }

    fn first(key: &SearchKey) -> String {
        let (first, rest) = wire(&uid_search(key));
        assert!(rest.is_empty(), "{rest:?}");
        first
    }

    fn text(s: &str) -> SearchKey {
        SearchKey::Text(s.into())
    }

    #[test]
    fn every_key() {
        let day = Day {
            year: 2024,
            month: 2,
            day: 1,
        };
        let cases = [
            (text("rota"), r#"TEXT "rota""#),
            (SearchKey::From("Ada".into()), r#"FROM "Ada""#),
            (SearchKey::To("a@x".into()), r#"TO "a@x""#),
            (SearchKey::Cc("a@x".into()), r#"CC "a@x""#),
            (SearchKey::Bcc("a@x".into()), r#"BCC "a@x""#),
            (
                SearchKey::Subject("Hi there".into()),
                r#"SUBJECT "Hi there""#,
            ),
            (SearchKey::Seen, "SEEN"),
            (SearchKey::Unseen, "UNSEEN"),
            (SearchKey::Flagged, "FLAGGED"),
            (SearchKey::Unflagged, "UNFLAGGED"),
            (SearchKey::Since(day), "SINCE 1-Feb-2024"),
            (
                SearchKey::Before(Day {
                    year: 2025,
                    month: 12,
                    day: 31,
                }),
                "BEFORE 31-Dec-2025",
            ),
            (SearchKey::Larger(10_000), "LARGER 10000"),
            (SearchKey::Smaller(u64::MAX), "SMALLER 4294967295"),
            (SearchKey::All(Vec::new()), "ALL"),
        ];
        for (key, sent) in cases {
            assert_eq!(first(&key), format!("UID SEARCH {sent}"));
        }
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(
            first(&text(r#"say "hi" \o/"#)),
            r#"UID SEARCH TEXT "say \"hi\" \\o/""#
        );
        assert_eq!(first(&text("a\0b")), r#"UID SEARCH TEXT "ab""#);
    }

    #[test]
    fn or_not_and_groups_nest() {
        let key = SearchKey::All(vec![
            text("rota"),
            SearchKey::negated(SearchKey::either(
                SearchKey::From("a".into()),
                SearchKey::All(vec![SearchKey::Seen, SearchKey::Flagged]),
            )),
            SearchKey::either(
                SearchKey::either(SearchKey::To("b".into()), SearchKey::Cc("b".into())),
                SearchKey::Bcc("b".into()),
            ),
        ]);
        assert_eq!(
            first(&key),
            r#"UID SEARCH (TEXT "rota" NOT OR FROM "a" (SEEN FLAGGED) OR OR TO "b" CC "b" BCC "b")"#
        );
        // A group of one is the key itself.
        assert_eq!(
            first(&SearchKey::All(vec![SearchKey::Seen])),
            "UID SEARCH SEEN"
        );
    }

    #[test]
    fn text_that_is_not_ascii_is_a_literal_in_utf8() {
        let key = SearchKey::All(vec![text("Grüße"), SearchKey::From("ann".into())]);
        let command = uid_search(&key);
        let (first, rest) = wire(&command);
        assert_eq!(first, "UID SEARCH CHARSET UTF-8 (TEXT {7+}");
        assert_eq!(rest, [b"Gr\xc3\xbc\xc3\x9fe FROM \"ann\")".to_vec()]);
        // Without LITERAL+, the literal waits for the server's `+`.
        let waiting = command.wire(false, false);
        assert_eq!(waiting.first, "UID SEARCH CHARSET UTF-8 (TEXT {7}");
        assert!(matches!(waiting.rest.as_slice(), [Line { wait: true, .. }]));
        // Line breaks cannot be quoted either, but are ASCII.
        let (first, rest) = wire(&uid_search(&text("a\r\nb")));
        assert_eq!(
            (first.as_str(), rest),
            ("UID SEARCH TEXT {4+}", vec![b"a\r\nb".to_vec()])
        );
    }

    #[test]
    fn gmails_language_goes_whole() {
        assert_eq!(
            wire(&gmail_raw(r#"from:ann "order shipped" has:attachment"#)).0,
            r#"UID SEARCH X-GM-RAW "from:ann \"order shipped\" has:attachment""#
        );
        let (first, rest) = wire(&gmail_raw("Grüße"));
        assert_eq!(first, "UID SEARCH CHARSET UTF-8 X-GM-RAW {7+}");
        assert_eq!(rest, ["Grüße".as_bytes().to_vec()]);
    }
}
