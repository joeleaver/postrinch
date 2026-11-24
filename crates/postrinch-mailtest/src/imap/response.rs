//! Writing IMAP response data: strings, flag lists, dates, and the
//! arguments commands take that are not plain values (dates, flag lists).

use std::collections::{BTreeSet, VecDeque};

use chrono::{DateTime, FixedOffset, NaiveDate};

use super::mailbox::canonical_flag;
use super::parser::Value;

/// Appends `bytes` as an IMAP string: quoted when it can be, a literal
/// otherwise (8-bit text, CR or LF, or long values).
pub(crate) fn push_string(out: &mut Vec<u8>, bytes: &[u8]) {
    let quotable = bytes.len() < 1000 && bytes.iter().all(|&b| (b' '..0x7f).contains(&b));
    if quotable {
        out.push(b'"');
        for &b in bytes {
            if b == b'"' || b == b'\\' {
                out.push(b'\\');
            }
            out.push(b);
        }
        out.push(b'"');
    } else {
        push_literal(out, bytes);
    }
}

/// Appends `bytes` as a literal.
pub(crate) fn push_literal(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(format!("{{{}}}\r\n", bytes.len()).as_bytes());
    out.extend_from_slice(bytes);
}

/// `(\Seen \Flagged)`.
pub(crate) fn flag_list<'a>(flags: impl IntoIterator<Item = &'a String>) -> String {
    let names: Vec<&str> = flags.into_iter().map(String::as_str).collect();
    format!("({})", names.join(" "))
}

/// An INTERNALDATE: `"24-Nov-2026 09:05:00 +0000"`.
pub(crate) fn format_datetime(date: &DateTime<FixedOffset>) -> String {
    format!("\"{}\"", date.format("%d-%b-%Y %H:%M:%S %z"))
}

/// Parses an APPEND date-time: `24-Nov-2026 09:05:00 +0000`, the day
/// padded with a space or a zero or not at all.
pub(crate) fn parse_datetime(text: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_str(text.trim_start(), "%d-%b-%Y %H:%M:%S %z").ok()
}

/// Parses a SEARCH date: `1-Feb-1994`.
pub(crate) fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%d-%b-%Y").ok()
}

/// A command's arguments, taken in order. Errors are the text of a BAD.
pub(crate) struct Args(VecDeque<Value>);

pub(crate) type ArgResult<T> = Result<T, String>;

impl Args {
    pub(crate) fn new(values: Vec<Value>) -> Self {
        Self(values.into())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn peek(&self) -> Option<&Value> {
        self.0.front()
    }

    pub(crate) fn next(&mut self) -> ArgResult<Value> {
        self.0
            .pop_front()
            .ok_or_else(|| "Missing argument".to_string())
    }

    /// The rest of the arguments.
    pub(crate) fn rest(&mut self) -> Vec<Value> {
        self.0.drain(..).collect()
    }

    pub(crate) fn atom(&mut self) -> ArgResult<String> {
        match self.next()? {
            Value::Atom(a) => Ok(a),
            other => Err(format!("Expected an atom, got {other:?}")),
        }
    }

    /// An astring (atom, quoted string or literal) as UTF-8 text.
    pub(crate) fn text(&mut self) -> ArgResult<String> {
        let value = self.next()?;
        let bytes = value
            .as_astring()
            .ok_or_else(|| "Expected a string".to_string())?;
        String::from_utf8(bytes.to_vec()).map_err(|_| "String is not UTF-8".to_string())
    }

    pub(crate) fn number(&mut self) -> ArgResult<u64> {
        let atom = self.atom()?;
        atom.parse()
            .map_err(|_| format!("Expected a number, got {atom}"))
    }

    pub(crate) fn list(&mut self) -> ArgResult<Vec<Value>> {
        match self.next()? {
            Value::List(l) => Ok(l),
            other => Err(format!("Expected a list, got {other:?}")),
        }
    }

    /// Takes a parenthesized list if one comes next.
    pub(crate) fn optional_list(&mut self) -> Option<Vec<Value>> {
        match self.peek() {
            Some(Value::List(_)) => self.list().ok(),
            _ => None,
        }
    }

    /// Fails if any argument is left over.
    pub(crate) fn end(&self) -> ArgResult<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err("Too many arguments".to_string())
        }
    }
}

/// Parses flags given as a list or as atoms, in canonical spelling.
pub(crate) fn parse_flags(values: &[Value]) -> ArgResult<BTreeSet<String>> {
    values
        .iter()
        .map(|v| {
            let atom = v
                .as_atom()
                .ok_or_else(|| "Flags must be atoms".to_string())?;
            canonical_flag(atom).ok_or_else(|| format!("Flag {atom} cannot be set"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings() {
        let mut out = Vec::new();
        push_string(&mut out, b"a \"b\" \\");
        assert_eq!(out, b"\"a \\\"b\\\" \\\\\"");
        out.clear();
        push_string(&mut out, "é".as_bytes());
        assert_eq!(out, b"{2}\r\n\xc3\xa9");
    }

    #[test]
    fn dates() {
        let d = parse_datetime(" 4-Nov-2026 09:05:00 +0100").unwrap();
        assert_eq!(format_datetime(&d), "\"04-Nov-2026 09:05:00 +0100\"");
        assert!(parse_datetime("04-Nov-2026 09:05:00 +0100").is_some());
        assert_eq!(
            parse_date("1-Feb-1994"),
            NaiveDate::from_ymd_opt(1994, 2, 1)
        );
        assert!(parse_date("1994-02-01").is_none());
    }
}
