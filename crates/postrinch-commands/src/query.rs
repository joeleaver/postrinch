//! The search language: words and `"phrases"` with Gmail's operators, so
//! what a person types here means what it means in Gmail, and the same
//! text can be handed to Gmail for mail not downloaded yet.
//!
//! `from:` `to:` `cc:` `bcc:` `subject:` `label:` `in:` `category:` `is:`
//! `has:` `after:` `before:` `newer_than:` `older_than:`, and `account:`,
//! which is Postrinch's own. A value with spaces is quoted
//! (`from:"Ada Lovelace"`); `-label:` and `-in:` leave a label out.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp as Instant, ToSpan, Zoned};
use postrinch_model::{SystemLabel, Timestamp};

use crate::{CommandError, CommandResult};

/// A query, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Words, phrases (quoted) and `prefix*` terms, as typed.
    pub words: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub subject: Vec<String>,
    /// `label:`/`in:`/`category:` values, each a label to find in every
    /// account.
    pub labels: Vec<LabelName>,
    pub without: Vec<LabelName>,
    pub accounts: Vec<String>,
    pub unread: Option<bool>,
    pub starred: Option<bool>,
    pub attachments: Option<bool>,
    pub after: Option<Timestamp>,
    pub before: Option<Timestamp>,
    /// `in:trash`, `in:spam` or `in:anywhere`: look where mailboxes don't.
    pub set_aside_too: bool,
    /// The query to give Gmail: what was typed, less Postrinch's own
    /// operators.
    pub gmail: String,
}

/// A label a query names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelName {
    System(SystemLabel),
    /// A person's label, by name as typed.
    Named(String),
}

impl Query {
    /// Read `text` in the search language, dates relative to `now`.
    pub fn parse(text: &str, now: Timestamp) -> CommandResult<Self> {
        let mut query = Query::default();
        let mut gmail = Vec::new();
        for token in tokens(text)? {
            let Some((negated, operator, value)) = operator(&token) else {
                query.words.push(token.clone());
                gmail.push(token);
                continue;
            };
            if operator != "account" {
                gmail.push(token.clone());
            }
            let value = unquote(value);
            if value.is_empty() {
                return Err(invalid(format!("{operator}: needs something after it.")));
            }
            if negated && !matches!(operator, "label" | "in" | "category") {
                return Err(invalid(format!(
                    "Leaving out with -{operator}: is not supported yet; -label: and -in: are."
                )));
            }
            match operator {
                "from" => query.from.push(value),
                "to" | "cc" | "bcc" => query.to.push(value),
                "subject" => query.subject.push(value),
                "account" => query.accounts.push(value),
                "label" | "in" | "category" => {
                    let label = query.label(operator, &value)?;
                    match (negated, label) {
                        (_, None) => {}
                        (false, Some(l)) => query.labels.push(l),
                        (true, Some(l)) => query.without.push(l),
                    }
                }
                "is" => match value.to_lowercase().as_str() {
                    "unread" => query.unread = Some(true),
                    "read" => query.unread = Some(false),
                    "starred" => query.starred = Some(true),
                    "unstarred" => query.starred = Some(false),
                    "important" => query.labels.push(LabelName::System(SystemLabel::Important)),
                    other => return Err(invalid(format!("is:{other} is not known."))),
                },
                "has" => match value.to_lowercase().as_str() {
                    "attachment" | "attachments" => query.attachments = Some(true),
                    other => return Err(invalid(format!("has:{other} is not known."))),
                },
                "after" => query.after = Some(date(&value)?),
                "before" => query.before = Some(date(&value)?),
                "newer_than" => query.after = Some(ago(&value, now)?),
                "older_than" => query.before = Some(ago(&value, now)?),
                other => return Err(invalid(format!("{other}: is not known."))),
            }
        }
        query.gmail = gmail.join(" ");
        Ok(query)
    }

    /// Whether the query has words (not only operators).
    pub fn has_words(&self) -> bool {
        !self.words.is_empty()
    }

    /// The words as rhypedb's full-text query: every word and phrase
    /// required, as in Gmail.
    pub fn text(&self) -> Option<String> {
        let terms: Vec<String> = self.words.iter().map(|w| format!("+{w}")).collect();
        (!terms.is_empty()).then(|| terms.join(" "))
    }

    /// The words to mark in an excerpt, lowercased.
    pub fn terms(&self) -> Vec<String> {
        self.words
            .iter()
            .flat_map(|w| w.split(|c: char| c.is_whitespace() || c == '"'))
            .map(|w| w.trim_end_matches('*'))
            .filter(|w| w.chars().count() > 1)
            .map(str::to_lowercase)
            .collect()
    }

    /// What `in:`, `category:` or `label:` names. `None` for `in:anywhere`.
    fn label(&mut self, operator: &str, value: &str) -> CommandResult<Option<LabelName>> {
        let lower = value.to_lowercase();
        if operator == "category" {
            let category = match lower.as_str() {
                "primary" | "personal" => SystemLabel::CategoryPersonal,
                "social" => SystemLabel::CategorySocial,
                "promotions" => SystemLabel::CategoryPromotions,
                "updates" => SystemLabel::CategoryUpdates,
                "forums" => SystemLabel::CategoryForums,
                other => return Err(invalid(format!("category:{other} is not known."))),
            };
            return Ok(Some(LabelName::System(category)));
        }
        let system = match lower.as_str() {
            "inbox" => SystemLabel::Inbox,
            "sent" => SystemLabel::Sent,
            "drafts" | "draft" => SystemLabel::Drafts,
            "important" => SystemLabel::Important,
            "trash" => {
                self.set_aside_too = true;
                SystemLabel::Trash
            }
            "spam" => {
                self.set_aside_too = true;
                SystemLabel::Spam
            }
            "starred" if operator == "in" => {
                self.starred = Some(true);
                return Ok(None);
            }
            "anywhere" if operator == "in" => {
                self.set_aside_too = true;
                return Ok(None);
            }
            _ => return Ok(Some(LabelName::Named(value.to_owned()))),
        };
        Ok(Some(LabelName::System(system)))
    }
}

/// A label's name as Gmail's `label:` matches it: any case, with spaces
/// and slashes written as hyphens (`label:work-clients` is "Work/Clients").
pub fn label_matches(name: &str, typed: &str) -> bool {
    let fold = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                ' ' | '/' | '_' => '-',
                c => c,
            })
            .flat_map(char::to_lowercase)
            .collect()
    };
    fold(name) == fold(typed)
}

/// Split into words, quoted phrases and `op:value` (whose value may be
/// quoted), keeping quotes.
pub(crate) fn tokens(text: &str) -> CommandResult<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if quoted {
        return Err(invalid("A quote is not closed.".into()));
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

/// `op:value` or `-op:value`, if the token is one.
pub(crate) fn operator(token: &str) -> Option<(bool, &str, &str)> {
    let (negated, rest) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let (name, value) = rest.split_once(':')?;
    let known = [
        "from",
        "to",
        "cc",
        "bcc",
        "subject",
        "label",
        "in",
        "category",
        "is",
        "has",
        "after",
        "before",
        "newer_than",
        "older_than",
        "account",
    ];
    let name = known.iter().find(|k| k.eq_ignore_ascii_case(name))?;
    Some((negated, name, value))
}

fn unquote(value: &str) -> String {
    value.trim_matches('"').trim().to_owned()
}

/// `2024/01/31` or `2024-01-31`, as the start of that day here.
fn date(value: &str) -> CommandResult<Timestamp> {
    let normal = value.replace('/', "-");
    let mut parts = normal.split('-').map(str::parse::<i16>);
    let (Some(Ok(y)), Some(Ok(m)), Some(Ok(d)), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid(format!("{value} is not a date like 2024/01/31.")));
    };
    let day = Date::new(y, m as i8, d as i8)
        .map_err(|_| invalid(format!("{value} is not a date like 2024/01/31.")))?;
    let zoned = day
        .to_zoned(TimeZone::system())
        .map_err(|e| invalid(format!("{value}: {e}")))?;
    Ok(Timestamp::from_millis(zoned.timestamp().as_millisecond()))
}

/// `3d`, `2w`, `6m` or `1y` before `now`.
fn ago(value: &str, now: Timestamp) -> CommandResult<Timestamp> {
    let bad = || invalid(format!("{value} is not an age like 3d, 2w, 6m or 1y."));
    let (n, unit) = value.split_at(value.len().saturating_sub(1));
    let n: i64 = n.parse().map_err(|_| bad())?;
    let span: Span = match unit {
        "d" => n.days(),
        "w" => n.weeks(),
        "m" => n.months(),
        "y" => n.years(),
        _ => return Err(bad()),
    };
    let now = Instant::from_millisecond(now.as_millis()).map_err(|_| bad())?;
    let then: Zoned = now
        .to_zoned(TimeZone::system())
        .checked_sub(span)
        .map_err(|_| bad())?;
    Ok(Timestamp::from_millis(then.timestamp().as_millisecond()))
}

fn invalid(message: String) -> CommandError {
    CommandError::Invalid(message)
}

#[cfg(test)]
mod tests {
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
    fn quoted_values_and_gmail_text() {
        let q = parse(r#"from:"Ada Lovelace" account:ada@example.org shift"#);
        assert_eq!(q.from, ["Ada Lovelace"]);
        assert_eq!(q.accounts, ["ada@example.org"]);
        assert_eq!(q.gmail, r#"from:"Ada Lovelace" shift"#);
        assert!(q.has_words());
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
    fn mistakes_say_what_is_wrong() {
        let now = Timestamp::from_millis(0);
        for bad in [
            "is:snoozed",
            "after:yesterday",
            "older_than:3q",
            "\"open",
            "-from:x",
            "from:",
        ] {
            assert!(Query::parse(bad, now).is_err(), "{bad}");
        }
        // An unknown word with a colon is just a word.
        assert_eq!(parse("re:invoice").words, ["re:invoice"]);
    }

    #[test]
    fn labels_match_as_gmail_writes_them() {
        assert!(label_matches("Work/Clients", "work-clients"));
        assert!(label_matches("Receipts 2024", "receipts-2024"));
        assert!(!label_matches("Work", "work-clients"));
    }
}
