//! SEARCH and UID SEARCH.
//!
//! Supported keys: ALL, the flag keys (SEEN, UNSEEN, FLAGGED, ...,
//! KEYWORD, UNKEYWORD, NEW, OLD, RECENT), a sequence set, UID, HEADER,
//! SUBJECT, FROM, TO, CC, BCC, BODY, TEXT, SINCE, BEFORE, ON, SENTSINCE,
//! SENTBEFORE, SENTON, LARGER, SMALLER, MODSEQ, NOT, OR and parenthesized
//! groups, after an optional `CHARSET UTF-8` / `US-ASCII`. String matches
//! are case-insensitive substrings of the raw (undecoded) text.

use chrono::{DateTime, NaiveDate};

use super::config::Extension;
use super::mailbox::{Message, canonical_flag};
use super::message::{contains_ignore_case, header_values, split_message};
use super::parser::Value;
use super::response::{ArgResult, Args, parse_date};
use super::seqset::SeqSet;
use super::session::{Reply, Session};

#[derive(Debug, Clone)]
enum Key {
    All,
    Flag(String, bool),
    Seq(SeqSet),
    Uid(SeqSet),
    Header(String, Vec<u8>),
    Body(Vec<u8>),
    Text(Vec<u8>),
    Since(NaiveDate),
    Before(NaiveDate),
    On(NaiveDate),
    SentSince(NaiveDate),
    SentBefore(NaiveDate),
    SentOn(NaiveDate),
    Larger(u64),
    Smaller(u64),
    ModSeq(u64),
    Not(Box<Key>),
    Or(Box<Key>, Box<Key>),
    And(Vec<Key>),
}

fn string(args: &mut Args) -> ArgResult<Vec<u8>> {
    let value = args.next()?;
    value
        .as_astring()
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "Expected a string".to_string())
}

fn date(args: &mut Args) -> ArgResult<NaiveDate> {
    let text = String::from_utf8(string(args)?).map_err(|_| "Bad date")?;
    parse_date(&text).ok_or_else(|| format!("Bad date {text}"))
}

fn parse_key(args: &mut Args) -> ArgResult<Key> {
    let value = args.next()?;
    let atom = match value {
        Value::List(list) => return parse_all(Args::new(list)),
        Value::Str(_) => return Err("Expected a search key".into()),
        Value::Atom(a) => a,
    };
    let upper = atom.to_ascii_uppercase();
    let flag = |name: &str, set: bool| Key::Flag(name.to_string(), set);
    Ok(match upper.as_str() {
        "ALL" | "OLD" => Key::All,
        "RECENT" => Key::Not(Box::new(Key::All)),
        "NEW" | "UNSEEN" => flag("\\Seen", false),
        "SEEN" => flag("\\Seen", true),
        "ANSWERED" => flag("\\Answered", true),
        "UNANSWERED" => flag("\\Answered", false),
        "DELETED" => flag("\\Deleted", true),
        "UNDELETED" => flag("\\Deleted", false),
        "DRAFT" => flag("\\Draft", true),
        "UNDRAFT" => flag("\\Draft", false),
        "FLAGGED" => flag("\\Flagged", true),
        "UNFLAGGED" => flag("\\Flagged", false),
        "KEYWORD" | "UNKEYWORD" => {
            let name = args.atom()?;
            let name = canonical_flag(&name).ok_or("Bad keyword")?;
            Key::Flag(name, upper == "KEYWORD")
        }
        "UID" => Key::Uid(SeqSet::parse(&args.atom()?).ok_or("Bad UID set")?),
        "HEADER" => {
            let name = String::from_utf8(string(args)?).map_err(|_| "Bad header name")?;
            Key::Header(name, string(args)?)
        }
        "SUBJECT" | "FROM" | "TO" | "CC" | "BCC" => Key::Header(upper.clone(), string(args)?),
        "BODY" => Key::Body(string(args)?),
        "TEXT" => Key::Text(string(args)?),
        "SINCE" => Key::Since(date(args)?),
        "BEFORE" => Key::Before(date(args)?),
        "ON" => Key::On(date(args)?),
        "SENTSINCE" => Key::SentSince(date(args)?),
        "SENTBEFORE" => Key::SentBefore(date(args)?),
        "SENTON" => Key::SentOn(date(args)?),
        "LARGER" => Key::Larger(args.number()?),
        "SMALLER" => Key::Smaller(args.number()?),
        "MODSEQ" => Key::ModSeq(args.number()?),
        "NOT" => Key::Not(Box::new(parse_key(args)?)),
        "OR" => {
            let a = parse_key(args)?;
            let b = parse_key(args)?;
            Key::Or(Box::new(a), Box::new(b))
        }
        _ => match SeqSet::parse(&atom) {
            Some(set) => Key::Seq(set),
            None => {
                return Err(format!(
                    "Search key {atom} is not supported by the fake server"
                ));
            }
        },
    })
}

fn parse_all(mut args: Args) -> ArgResult<Key> {
    let mut keys = Vec::new();
    while !args.is_empty() {
        keys.push(parse_key(&mut args)?);
    }
    if keys.is_empty() {
        return Err("Missing search key".into());
    }
    Ok(Key::And(keys))
}

fn uses_modseq(key: &Key) -> bool {
    match key {
        Key::ModSeq(_) => true,
        Key::Not(k) => uses_modseq(k),
        Key::Or(a, b) => uses_modseq(a) || uses_modseq(b),
        Key::And(keys) => keys.iter().any(uses_modseq),
        _ => false,
    }
}

/// Where a message is, for keys that need it.
struct Place {
    seq: u32,
    largest_seq: u32,
    largest_uid: u32,
}

fn sent_date(message: &Message) -> Option<NaiveDate> {
    let value = header_values(&message.body, "Date").into_iter().next()?;
    DateTime::parse_from_rfc2822(&value)
        .ok()
        .map(|d| d.date_naive())
}

fn matches(key: &Key, message: &Message, place: &Place) -> bool {
    let date = message.internal_date.date_naive();
    match key {
        Key::All => true,
        Key::Flag(name, set) => message.flags.contains(name) == *set,
        Key::Seq(s) => s.contains(place.seq, place.largest_seq),
        Key::Uid(s) => s.contains(message.uid, place.largest_uid),
        Key::Header(name, needle) => {
            let values = header_values(&message.body, name);
            if needle.is_empty() {
                !values.is_empty()
            } else {
                values
                    .iter()
                    .any(|v| contains_ignore_case(v.as_bytes(), needle))
            }
        }
        Key::Body(needle) => contains_ignore_case(split_message(&message.body).1, needle),
        Key::Text(needle) => contains_ignore_case(&message.body, needle),
        Key::Since(d) => date >= *d,
        Key::Before(d) => date < *d,
        Key::On(d) => date == *d,
        Key::SentSince(d) => sent_date(message).is_some_and(|s| s >= *d),
        Key::SentBefore(d) => sent_date(message).is_some_and(|s| s < *d),
        Key::SentOn(d) => sent_date(message) == Some(*d),
        Key::Larger(n) => message.body.len() as u64 > *n,
        Key::Smaller(n) => (message.body.len() as u64) < *n,
        Key::ModSeq(n) => message.modseq >= *n,
        Key::Not(k) => !matches(k, message, place),
        Key::Or(a, b) => matches(a, message, place) || matches(b, message, place),
        Key::And(keys) => keys.iter().all(|k| matches(k, message, place)),
    }
}

impl Session {
    pub(crate) fn search(&mut self, mut args: Args, uid_mode: bool) -> Reply {
        if let Some(Value::Atom(a)) = args.peek()
            && a.eq_ignore_ascii_case("CHARSET")
        {
            let _ = args.next();
            match args.text() {
                Ok(c) if c.eq_ignore_ascii_case("UTF-8") || c.eq_ignore_ascii_case("US-ASCII") => {}
                Ok(_) => {
                    return Reply::No("[BADCHARSET (UTF-8 US-ASCII)] Unsupported charset".into());
                }
                Err(e) => return Reply::Bad(e),
            }
        }
        let key = match parse_all(args) {
            Ok(k) => k,
            Err(e) => return Reply::Bad(e),
        };
        let modseq = uses_modseq(&key);
        if modseq {
            if !self.shared.lock().has(Extension::Condstore) {
                return Reply::Bad("CONDSTORE is not offered".into());
            }
            self.condstore = true;
        }
        self.sync_selected(false);
        let user = self.user.clone().unwrap_or_default();
        let state = self.shared.lock();
        let (Some(selected), Some(account)) = (self.selected.as_ref(), state.accounts.get(&user))
        else {
            return Reply::No("No mailbox selected".into());
        };
        let Some(folder) = account.folder_by_id(selected.folder_id) else {
            return Reply::No("[NONEXISTENT] The mailbox is gone".into());
        };
        let place = |seq: usize| Place {
            seq: u32::try_from(seq).unwrap_or(u32::MAX),
            largest_seq: u32::try_from(selected.uids.len()).unwrap_or(u32::MAX),
            largest_uid: selected.uids.last().copied().unwrap_or(0),
        };
        let mut found = Vec::new();
        let mut highest = 0;
        for (i, uid) in selected.uids.iter().enumerate() {
            let Some(message) = folder.find(*uid) else {
                continue;
            };
            if matches(&key, message, &place(i + 1)) {
                found.push(if uid_mode { *uid as usize } else { i + 1 });
                highest = highest.max(message.modseq);
            }
        }
        let mut line = String::from("* SEARCH");
        for n in &found {
            line.push_str(&format!(" {n}"));
        }
        if modseq && !found.is_empty() {
            line.push_str(&format!(" (MODSEQ {highest})"));
        }
        line.push_str("\r\n");
        self.out.extend_from_slice(line.as_bytes());
        Reply::Ok("SEARCH completed".into())
    }
}
