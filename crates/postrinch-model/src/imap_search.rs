//! A search to ask an IMAP server, made from the search language for mail
//! not downloaded yet (docs/imap.md, "Searching the server").
//!
//! The command layer reads what the person typed and writes it here: per
//! alternative of the query (`a OR b` spreads into alternatives), the
//! folders it looks in and the IMAP `SEARCH` keys (RFC 3501 6.4.4) that
//! say what it asks. Sync chooses the folders from the account's roles,
//! writes the keys on the wire (quoted strings, literals, `CHARSET
//! UTF-8`) and fetches what is found.
//!
//! Every key asks for at least what the query asks: a term IMAP cannot
//! say exactly is said more loosely (a date a day or two wider) or left
//! out, and the command layer checks what is found against the whole
//! query again before it is shown. The one exception is a term neither
//! the server nor the headers here can check (`has:attachment`), which
//! the alternative names in [`ImapAlternative::inexact`]: only Gmail's
//! own language (`X-GM-RAW`) can ask it, and other servers refuse the
//! search, saying so, rather than answer it wrongly.

use serde::{Deserialize, Serialize};

use crate::Day;

/// One IMAP search key, or keys joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchKey {
    /// `TEXT`: in the headers or the body.
    Text(String),
    From(String),
    To(String),
    Cc(String),
    Bcc(String),
    Subject(String),
    Seen,
    Unseen,
    Flagged,
    Unflagged,
    /// `SINCE`: arrived on or after the day (by the server's date of it).
    Since(Day),
    /// `BEFORE`: arrived before the day.
    Before(Day),
    /// `LARGER`: more than this many bytes.
    Larger(u64),
    /// `SMALLER`: fewer than this many bytes.
    Smaller(u64),
    Not(Box<SearchKey>),
    Or(Box<SearchKey>, Box<SearchKey>),
    /// Every key holds; none is every message (`ALL`).
    All(Vec<SearchKey>),
}

impl SearchKey {
    /// Either of two keys.
    pub fn either(a: SearchKey, b: SearchKey) -> Self {
        Self::Or(Box::new(a), Box::new(b))
    }

    /// Not the key.
    pub fn negated(key: SearchKey) -> Self {
        Self::Not(Box::new(key))
    }

    /// Any of these keys, as nested `OR`s; `None` for none.
    pub fn any(keys: impl IntoIterator<Item = SearchKey>) -> Option<Self> {
        let mut keys = keys.into_iter();
        let first = keys.next()?;
        Some(keys.fold(first, SearchKey::either))
    }

    /// Whether the key holds for every message.
    pub fn is_every(&self) -> bool {
        matches!(self, Self::All(keys) if keys.iter().all(SearchKey::is_every))
    }
}

/// A search of one account's server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapSearch {
    /// What is found by any alternative is found.
    pub alternatives: Vec<ImapAlternative>,
}

/// One alternative of a search: where it looks and what it asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapAlternative {
    /// The folders (label ids) to look in: mail must be in one of them.
    /// None for the folders a search covers (every folder but Trash and
    /// Junk, or Gmail's All Mail).
    pub folders: Vec<String>,
    /// Folders (label ids) not to look in.
    pub not_in: Vec<String>,
    /// Trash and Junk are looked in too.
    pub set_aside_too: bool,
    pub key: SearchKey,
    /// The alternative in Gmail's search language, for Gmail's servers
    /// (`X-GM-RAW`).
    pub gmail: String,
    /// What `key` leaves out that nothing but Gmail's language can ask,
    /// in the search language (`has:attachment`); `None` when `key` says
    /// it all.
    pub inexact: Option<String>,
}
