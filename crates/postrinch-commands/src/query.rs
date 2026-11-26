//! The search language: words and `"phrases"` with Gmail's operators, so
//! what a person types here means what it means in Gmail, and the same
//! text can be handed to Gmail for mail not downloaded yet.
//!
//! `from:` `to:` `cc:` `bcc:` `subject:` `label:` `in:` `category:` `is:`
//! `has:` `after:` `before:` `newer_than:` `older_than:` `larger:`
//! `smaller:` (`size:` is `larger:`, as in Gmail), and Postrinch's
//! own `account:` and `last:` (`last:me`, the account answered last;
//! `last:them`, someone else wrote last). A value with spaces is quoted
//! (`from:"Ada Lovelace"`); `-label:` and `-in:` leave a label out, and
//! `-from:` and `-to:` a sender or a recipient.

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
    /// `-from:`: senders it is not from.
    pub not_from: Vec<String>,
    /// `-to:` (or `-cc:`, `-bcc:`): recipients it is not to.
    pub not_to: Vec<String>,
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
    /// `larger:`: more than this many bytes.
    pub larger: Option<u64>,
    /// `smaller:`: fewer than this many bytes.
    pub smaller: Option<u64>,
    /// `last:me` or `last:them`: whose the conversation's latest message
    /// is.
    pub last: Option<Last>,
    /// `in:trash`, `in:spam` or `in:anywhere`: look where mailboxes don't.
    pub set_aside_too: bool,
    /// The query to give Gmail: what was typed, less Postrinch's own
    /// operators.
    pub gmail: String,
    /// [`Query::gmail`] less everything it leaves out (`-word`, `-from:`,
    /// `-to:`, `-cc:`, `-bcc:`, `-label:`, `-in:`, `-category:`): what an
    /// IMAP server speaking Gmail's language (`X-GM-RAW`) is asked. It is
    /// asked for at least what the query matches, as every IMAP server is
    /// (imap_search.rs), and Gmail matches its own way: it folds accents,
    /// so `-from:jose` would leave out "José", whom search here keeps, and
    /// it folds case, `/`, `-` and spaces in label names, so `-label:a-b`
    /// would leave out "a b" too. The check made on what it finds leaves
    /// them out as search here does.
    pub gmail_superset: String,
}

/// Who wrote a conversation's latest message, as `last:` asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Last {
    /// The account answered last: the person is done with it for now.
    Me,
    /// Someone else did: it is the person's turn.
    Them,
}

impl Last {
    /// Whether a conversation is one this asks for.
    pub fn holds(self, summary: &postrinch_model::ThreadSummary) -> bool {
        summary.replied_last.is_some() == (self == Last::Me)
    }
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
        let mut superset = Vec::new();
        for token in tokens(text)? {
            let Some((negated, operator, value)) = operator(&token) else {
                query.words.push(token.clone());
                if !token.starts_with('-') {
                    superset.push(token.clone());
                }
                gmail.push(token);
                continue;
            };
            if !matches!(operator, "account" | "last") {
                gmail.push(token.clone());
                if !negated {
                    superset.push(token.clone());
                }
            }
            let value = unquote(value);
            if value.is_empty() {
                return Err(invalid(format!("{operator}: needs something after it.")));
            }
            let leaves_out = matches!(
                operator,
                "label" | "in" | "category" | "from" | "to" | "cc" | "bcc"
            );
            if negated && !leaves_out {
                return Err(invalid(format!(
                    "Leaving out with -{operator}: is not supported yet; -label:, -in:, -from: and -to: are."
                )));
            }
            match operator {
                "from" if negated => query.not_from.push(value),
                "from" => query.from.push(value),
                "to" | "cc" | "bcc" if negated => query.not_to.push(value),
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
                "last" => match value.to_lowercase().as_str() {
                    "me" => query.last = Some(Last::Me),
                    "them" => query.last = Some(Last::Them),
                    other => {
                        return Err(invalid(format!(
                            "last:{other} is not known; last:me and last:them are."
                        )));
                    }
                },
                "after" => query.after = Some(date(&value)?),
                "before" => query.before = Some(date(&value)?),
                "newer_than" => query.after = Some(ago(&value, now)?),
                "older_than" => query.before = Some(ago(&value, now)?),
                "larger" | "size" => query.larger = Some(size(&value)?),
                "smaller" => query.smaller = Some(size(&value)?),
                other => return Err(invalid(format!("{other}: is not known."))),
            }
        }
        query.gmail = gmail.join(" ");
        query.gmail_superset = superset.join(" ");
        Ok(query)
    }

    /// Whether the query has words (not only operators).
    pub fn has_words(&self) -> bool {
        !self.words.is_empty()
    }

    /// The words and phrases mail must have, as typed: all but those
    /// with Gmail's `-` before them.
    pub fn wanted_words(&self) -> impl Iterator<Item = &str> {
        self.words
            .iter()
            .map(String::as_str)
            .filter(|w| !w.starts_with('-'))
    }

    /// The words and phrases mail must not have (Gmail's `-word`), without
    /// their `-`.
    pub fn unwanted_words(&self) -> impl Iterator<Item = &str> {
        self.words.iter().filter_map(|w| w.strip_prefix('-'))
    }

    /// The words mail must have as rhypedb's full-text query: every word
    /// and phrase required, as in Gmail.
    pub fn text(&self) -> Option<String> {
        let terms: Vec<String> = self.wanted_words().map(|w| format!("+{w}")).collect();
        (!terms.is_empty()).then(|| terms.join(" "))
    }

    /// The words mail must not have, each as rhypedb's full-text query
    /// that mail with it matches (every word of `-invoice-4471` or of a
    /// phrase, as the word without `-` would be searched for). rhypedb has
    /// no query that leaves out, so the store leaves out what these
    /// match. One that is no term to search for (stop words, punctuation)
    /// leaves nothing out.
    pub fn text_without(&self) -> Vec<String> {
        self.unwanted_words()
            .filter(|w| !postrinch_store::search_tokens(w).is_empty())
            .map(|w| format!("+{w}"))
            .collect()
    }

    /// The words to mark in an excerpt, lowercased: not those left out.
    pub fn terms(&self) -> Vec<String> {
        self.wanted_words()
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
        "larger",
        "smaller",
        "size",
        "account",
        "last",
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

/// `500`, `200K`, `10M` or `1G` (binary units, an optional `B` after
/// them, any case), in bytes.
fn size(value: &str) -> CommandResult<u64> {
    let bad = || invalid(format!("{value} is not a size like 500K or 10M."));
    let upper = value.to_ascii_uppercase();
    let digits = upper.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &upper[digits.len()..];
    let times: u64 = match unit {
        "" | "B" => 1,
        "K" | "KB" => 1 << 10,
        "M" | "MB" => 1 << 20,
        "G" | "GB" => 1 << 30,
        _ => return Err(bad()),
    };
    let n: u64 = digits.parse().map_err(|_| bad())?;
    n.checked_mul(times).ok_or_else(bad)
}

fn invalid(message: String) -> CommandError {
    CommandError::Invalid(message)
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
