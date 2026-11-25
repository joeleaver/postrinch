//! A query in the search language as an IMAP server's `SEARCH`
//! ([`ImapSearch`]), for the mail of an IMAP account not downloaded yet
//! (docs/imap.md, "Searching the server").
//!
//! Each alternative of the query (query_or.rs) becomes one
//! [`ImapAlternative`]: its labels the folders to look in, its terms the
//! keys. Every key asks for at least what the term asks, so nothing the
//! query matches is missed: words go by their stems' text ([`key`]),
//! dates go a few days wider (a server dates mail in its own zone), and
//! terms the headers here can answer but IMAP cannot ask (a second label,
//! `last:`, a word left out) are left to the check made on what is found
//! ([`Commands::remote_rows`]). The one term neither can answer,
//! `has:attachment`, is named in [`ImapAlternative::inexact`], so a server
//! without Gmail's own language refuses the search rather than answer it
//! wrongly.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp as Instant, ToSpan};
use postrinch_model::{
    Day, EmailAddress, ImapAlternative, ImapSearch, Label, LabelKind, SearchKey, Timestamp,
};

use postrinch_store::{search_tokens, server_texts};

use crate::query::{LabelName, Query, label_matches};
use crate::query_or::Either;
use crate::system_labels::system_label_in;
use crate::{CommandResult, Commands};

/// How many days wider than asked a date goes to the server: the server
/// dates mail by its own clock's day, up to 26 hours from this one's.
const DATE_SLACK: i64 = 2;

impl Commands {
    /// `query` (in the search language) as a search of `account`'s IMAP
    /// server. Alternatives that cannot match in the account (a label it
    /// does not have, `account:` naming another) are left out; none left
    /// is a search that finds nothing.
    pub fn imap_search(&self, query: &str, account: &EmailAddress) -> CommandResult<ImapSearch> {
        self.check(account)?;
        let now = Timestamp::from_millis(jiff::Timestamp::now().as_millisecond());
        let either = Either::parse(query.trim(), now)?;
        let labels = self.store.labels(account)?;
        let zone = TimeZone::system();
        let mut alternatives = Vec::new();
        for one in &either.queries {
            if !self.search_accounts(one)?.contains(account) {
                continue;
            }
            if let Some(alternative) = alternative(one, &labels, &zone) {
                alternatives.push(alternative);
            }
        }
        Ok(ImapSearch { alternatives })
    }
}

/// One alternative over an account with these labels; `None` when it
/// names a label the account does not have, so it matches nothing there.
pub(crate) fn alternative(
    query: &Query,
    labels: &[Label],
    zone: &TimeZone,
) -> Option<ImapAlternative> {
    if !names_only_labels_in(query, labels) {
        return None;
    }
    // The first label's folders are where to look; the others are checked
    // on what is found. Each label is a folder in an IMAP account.
    let folders = query
        .labels
        .first()
        .map(|name| label_ids(labels, name))
        .unwrap_or_default();
    let not_in = query
        .without
        .iter()
        .flat_map(|name| label_ids(labels, name))
        .collect();
    let (key, inexact) = key(query, zone);
    Some(ImapAlternative {
        folders,
        not_in,
        set_aside_too: query.set_aside_too,
        key,
        gmail: query.gmail.clone(),
        inexact,
    })
}

/// Whether every label the query must be in is one of these.
pub(crate) fn names_only_labels_in(query: &Query, labels: &[Label]) -> bool {
    query
        .labels
        .iter()
        .all(|n| !label_ids(labels, n).is_empty())
}

/// The ids of the labels a name names among one account's labels.
fn label_ids(labels: &[Label], name: &LabelName) -> Vec<String> {
    match name {
        LabelName::System(s) => system_label_in(labels, *s)
            .map(|l| l.provider_id.clone())
            .into_iter()
            .collect(),
        LabelName::Named(n) => labels
            .iter()
            .filter(|l| l.kind == LabelKind::User && label_matches(&l.name, n))
            .map(|l| l.provider_id.clone())
            .collect(),
    }
}

/// The query's terms as keys that hold for at least what it matches, and
/// the term no key can ask, if it has one. Dates are days in `zone`.
pub(crate) fn key(query: &Query, zone: &TimeZone) -> (SearchKey, Option<String>) {
    let mut keys = Vec::new();
    // Words are asked term by term, as full-text search here reads them
    // (`search_tokens`): `invoice-4471` is two terms, and a phrase is its
    // terms wherever they are, so a hard wrap or punctuation between its
    // words does not hide it, and a `prefix*` is its prefix. A term is
    // asked by text every word of its stem contains (`server_texts`), as
    // TEXT matches substrings: `shipping` asks for `ship`, which
    // `shipped` has too. Words left out (`-draft`) are not asked at all:
    // TEXT matches a part of a word in any header, so NOT TEXT would leave
    // out mail that search here keeps (`-cat` and "concatenate", or a word
    // in a `Received:` header); the check made on what is found leaves
    // them out as search here does.
    for word in query.wanted_words() {
        let word = word.replace(['"', '*'], " ");
        for token in search_tokens(&word) {
            let texts = server_texts(&token).into_iter().map(SearchKey::Text);
            if let Some(key) = SearchKey::any(texts)
                && !keys.contains(&key)
            {
                keys.push(key);
            }
        }
    }
    keys.extend(query.from.iter().map(|v| SearchKey::From(v.clone())));
    keys.extend(query.to.iter().map(|v| recipient(v)));
    keys.extend(
        query
            .not_from
            .iter()
            .map(|v| SearchKey::negated(SearchKey::From(v.clone()))),
    );
    keys.extend(
        query
            .not_to
            .iter()
            .map(|v| SearchKey::negated(recipient(v))),
    );
    keys.extend(query.subject.iter().map(|v| SearchKey::Subject(v.clone())));
    match query.unread {
        Some(true) => keys.push(SearchKey::Unseen),
        Some(false) => keys.push(SearchKey::Seen),
        None => {}
    }
    match query.starred {
        Some(true) => keys.push(SearchKey::Flagged),
        Some(false) => keys.push(SearchKey::Unflagged),
        None => {}
    }
    if let Some(day) = query.after.and_then(|t| day_of(t, zone, -DATE_SLACK)) {
        keys.push(SearchKey::Since(day));
    }
    if let Some(day) = query.before.and_then(|t| day_of(t, zone, DATE_SLACK + 1)) {
        keys.push(SearchKey::Before(day));
    }
    keys.extend(query.larger.map(SearchKey::Larger));
    keys.extend(query.smaller.map(SearchKey::Smaller));
    // Whether a message has attachments is in its body: neither IMAP's
    // keys nor the headers here can say.
    let inexact = query.attachments.map(|_| "has:attachment".to_owned());
    let key = match <[SearchKey; 1]>::try_from(keys) {
        Ok([one]) => one,
        Err(keys) => SearchKey::All(keys),
    };
    (key, inexact)
}

/// `to:` (and `cc:`, `bcc:`, which the search language reads as one): any
/// recipient.
fn recipient(value: &str) -> SearchKey {
    SearchKey::either(
        SearchKey::To(value.to_owned()),
        SearchKey::either(
            SearchKey::Cc(value.to_owned()),
            SearchKey::Bcc(value.to_owned()),
        ),
    )
}

/// The day `at` falls on in `zone`, moved by `shift` days; `None` past the
/// calendar's ends, where no date is asked.
fn day_of(at: Timestamp, zone: &TimeZone, shift: i64) -> Option<Day> {
    let instant = Instant::from_millisecond(at.as_millis()).ok()?;
    let date: Date = instant
        .to_zoned(zone.clone())
        .date()
        .checked_add(shift.days())
        .ok()?;
    Some(Day {
        year: date.year(),
        month: date.month(),
        day: date.day(),
    })
}

#[cfg(test)]
#[path = "imap_search_tests.rs"]
mod tests;
