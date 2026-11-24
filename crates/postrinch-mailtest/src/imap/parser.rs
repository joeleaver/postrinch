//! Reading IMAP commands off the wire and splitting them into values.
//!
//! A command is one line, except that it may carry literals: a line ending
//! in `{n}` (synchronizing: the server answers `+` before the client sends
//! the bytes) or `{n+}` (LITERAL+, RFC 7888: the bytes follow at once) is
//! followed by exactly `n` bytes of anything, CRLF included, and then the
//! rest of the command on a new line. [`read_command`] gathers those pieces
//! as [`Part`]s without ever looking inside a literal, and [`tokenize`]
//! turns them into [`Value`]s:
//!
//! - atoms (`FETCH`, `\Seen`, `1:*`, `BODY.PEEK[HEADER.FIELDS (From)]<0.10>`:
//!   a `[` inside an atom runs to its matching `]`, spaces and parentheses
//!   included, as RFC 3501's fetch attributes need);
//! - strings, quoted (with `\"` and `\\` escapes) or literal;
//! - parenthesized lists of values.

use std::fmt;
use std::io;

use crate::conn::Conn;

/// The largest literal the fake accepts (a message to APPEND, say).
pub(crate) const MAX_LITERAL: usize = 64 * 1024 * 1024;

/// One value in a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    /// An atom, as written (case preserved).
    Atom(String),
    /// A quoted string or a literal: bytes, not necessarily UTF-8.
    Str(Vec<u8>),
    /// A parenthesized list.
    List(Vec<Value>),
}

impl Value {
    /// The value as an atom, if it is one.
    pub(crate) fn as_atom(&self) -> Option<&str> {
        match self {
            Value::Atom(a) => Some(a),
            _ => None,
        }
    }

    /// The value as an astring (atom or string): its bytes.
    pub(crate) fn as_astring(&self) -> Option<&[u8]> {
        match self {
            Value::Atom(a) => Some(a.as_bytes()),
            Value::Str(s) => Some(s),
            Value::List(_) => None,
        }
    }
}

/// A piece of a command as it came off the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Part {
    /// Text of a line, without its line ending or trailing literal marker.
    Text(Vec<u8>),
    /// A literal's bytes.
    Literal(Vec<u8>),
}

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Command {
    pub(crate) tag: String,
    /// The command name, upper case; `UID FETCH` and the like for UID
    /// commands.
    pub(crate) name: String,
    pub(crate) args: Vec<Value>,
}

/// What reading the next command gave.
#[derive(Debug)]
pub(crate) enum Read {
    /// A command, with the text to log for it.
    Command(Command, String),
    /// A line that is not a valid command; answered with BAD. `tag` is the
    /// line's tag when one could be found.
    Invalid { tag: Option<String>, reason: String },
    /// The client closed the connection.
    Eof,
}

/// A syntax error in a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyntaxError(pub(crate) String);

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The literal marker at the end of a line: `(size, synchronizing)`.
fn literal_marker(line: &[u8]) -> Option<(usize, usize, bool)> {
    let body = line.strip_suffix(b"}")?;
    let open = body.iter().rposition(|&b| b == b'{')?;
    let mut digits = &body[open + 1..];
    let sync = match digits.strip_suffix(b"+") {
        Some(d) => {
            digits = d;
            false
        }
        None => true,
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let size = std::str::from_utf8(digits).ok()?.parse().ok()?;
    Some((open, size, sync))
}

/// Reads one whole command, literals and all, answering `+` for each
/// synchronizing literal.
pub(crate) async fn read_command(conn: &mut Conn) -> io::Result<Read> {
    let mut parts = Vec::new();
    loop {
        let Some(mut line) = conn.read_line().await? else {
            return Ok(Read::Eof);
        };
        match literal_marker(&line) {
            Some((open, size, sync)) => {
                line.truncate(open);
                parts.push(Part::Text(line));
                if size > MAX_LITERAL {
                    let tag = first_word(&parts);
                    return Ok(Read::Invalid {
                        tag,
                        reason: "Literal too large".into(),
                    });
                }
                if sync {
                    conn.write_all(b"+ Ready for literal data\r\n").await?;
                }
                let Some(bytes) = conn.read_exact(size).await? else {
                    return Ok(Read::Eof);
                };
                parts.push(Part::Literal(bytes));
            }
            None => {
                parts.push(Part::Text(line));
                break;
            }
        }
    }
    let log = log_text(&parts);
    Ok(match parse_command(parts) {
        Ok(command) => Read::Command(command, log),
        Err((tag, SyntaxError(reason))) => Read::Invalid { tag, reason },
    })
}

/// The first space-separated word of the command, used as the tag of a
/// BAD answer to a line that does not parse.
fn first_word(parts: &[Part]) -> Option<String> {
    let Some(Part::Text(text)) = parts.first() else {
        return None;
    };
    let word = text.split(|&b| b == b' ').next()?;
    let word = std::str::from_utf8(word).ok()?;
    (!word.is_empty() && word.bytes().all(is_tag_char)).then(|| word.to_string())
}

/// The command as logged: its text with each literal shown as `{n}`, and
/// without its tag.
fn log_text(parts: &[Part]) -> String {
    let mut log = String::new();
    for part in parts {
        match part {
            Part::Text(t) => log.push_str(&String::from_utf8_lossy(t)),
            Part::Literal(l) => log.push_str(&format!("{{{}}}", l.len())),
        }
    }
    match log.split_once(' ') {
        Some((_, rest)) => rest.to_string(),
        None => String::new(),
    }
}

fn is_tag_char(b: u8) -> bool {
    is_atom_char(b) && b != b'+'
}

/// RFC 3501 ATOM-CHAR, plus `]` (allowed in astrings) and the list
/// wildcards and `\` which flags and LIST patterns use.
fn is_atom_char(b: u8) -> bool {
    b > b' ' && b < 0x7f && !matches!(b, b'(' | b')' | b'{' | b'"')
}

/// Splits the pieces of a command into tag, name and arguments.
pub(crate) fn parse_command(parts: Vec<Part>) -> Result<Command, (Option<String>, SyntaxError)> {
    let tag = first_word(&parts);
    let mut values = tokenize(parts).map_err(|e| (tag.clone(), e))?.into_iter();
    let fail = |reason: &str| (tag.clone(), SyntaxError(reason.to_string()));
    let tag = match values.next() {
        Some(Value::Atom(t)) if t.bytes().all(is_tag_char) => t,
        _ => return Err(fail("Missing or invalid tag")),
    };
    let mut name = match values.next() {
        Some(Value::Atom(n)) => n.to_ascii_uppercase(),
        _ => return Err((Some(tag), SyntaxError("Missing command".into()))),
    };
    if name == "UID" {
        match values.next() {
            Some(Value::Atom(n)) => name = format!("UID {}", n.to_ascii_uppercase()),
            _ => return Err((Some(tag), SyntaxError("Missing UID command".into()))),
        }
    }
    Ok(Command {
        tag,
        name,
        args: values.collect(),
    })
}

/// Splits the pieces of a command into values.
pub(crate) fn tokenize(parts: Vec<Part>) -> Result<Vec<Value>, SyntaxError> {
    let mut tokenizer = Tokenizer {
        parts,
        part: 0,
        pos: 0,
    };
    let values = tokenizer.values(false)?;
    Ok(values)
}

struct Tokenizer {
    parts: Vec<Part>,
    part: usize,
    pos: usize,
}

impl Tokenizer {
    /// The text part being read and the position in it, if any.
    fn text(&self) -> Option<&[u8]> {
        match self.parts.get(self.part) {
            Some(Part::Text(t)) => Some(&t[self.pos.min(t.len())..]),
            _ => None,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text().and_then(|t| t.first().copied())
    }

    /// Reads values until the end of the command or, inside a list, its
    /// closing parenthesis (which is consumed).
    fn values(&mut self, in_list: bool) -> Result<Vec<Value>, SyntaxError> {
        let mut values = Vec::new();
        loop {
            while self.peek() == Some(b' ') {
                self.pos += 1;
            }
            match self.peek() {
                None => {
                    // The end of a text part: a literal follows, or the
                    // command ends.
                    match self.parts.get(self.part + 1) {
                        Some(Part::Literal(bytes)) => {
                            values.push(Value::Str(bytes.clone()));
                            self.part += 2;
                            self.pos = 0;
                        }
                        _ if in_list => return Err(SyntaxError("Unclosed list".into())),
                        _ => return Ok(values),
                    }
                }
                Some(b'(') => {
                    self.pos += 1;
                    values.push(Value::List(self.values(true)?));
                }
                Some(b')') => {
                    if !in_list {
                        return Err(SyntaxError("Unexpected )".into()));
                    }
                    self.pos += 1;
                    return Ok(values);
                }
                Some(b'"') => values.push(Value::Str(self.quoted()?)),
                Some(_) => values.push(Value::Atom(self.atom()?)),
            }
        }
    }

    fn quoted(&mut self) -> Result<Vec<u8>, SyntaxError> {
        let text = self.text().unwrap_or_default();
        let mut out = Vec::new();
        let mut i = 1;
        loop {
            match text.get(i) {
                None => return Err(SyntaxError("Unterminated quoted string".into())),
                Some(b'"') => break,
                Some(b'\\') => match text.get(i + 1) {
                    Some(&c @ (b'\\' | b'"')) => {
                        out.push(c);
                        i += 1;
                    }
                    _ => return Err(SyntaxError("Invalid escape in quoted string".into())),
                },
                Some(&c) => out.push(c),
            }
            i += 1;
        }
        self.pos += i + 1;
        Ok(out)
    }

    fn atom(&mut self) -> Result<String, SyntaxError> {
        let text = self.text().unwrap_or_default();
        let mut depth = 0usize;
        let mut i = 0;
        while let Some(&c) = text.get(i) {
            match c {
                b'[' => depth += 1,
                b']' if depth > 0 => depth -= 1,
                // Inside brackets anything printable goes, for
                // `BODY[HEADER.FIELDS (From To)]`.
                _ if depth > 0 && (b' '..0x7f).contains(&c) => {}
                _ if !is_atom_char(c) => break,
                _ => {}
            }
            i += 1;
        }
        if depth > 0 {
            return Err(SyntaxError("Unclosed [ in atom".into()));
        }
        if i == 0 {
            return Err(SyntaxError(format!(
                "Unexpected character {:?}",
                char::from(text[0])
            )));
        }
        let atom = String::from_utf8(text[..i].to_vec())
            .map_err(|_| SyntaxError("Atom is not ASCII".into()))?;
        self.pos += i;
        Ok(atom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Part {
        Part::Text(s.as_bytes().to_vec())
    }

    fn atom(s: &str) -> Value {
        Value::Atom(s.into())
    }

    #[test]
    fn markers() {
        assert_eq!(literal_marker(b"A APPEND x {12}"), Some((11, 12, true)));
        assert_eq!(literal_marker(b"A APPEND x {12+}"), Some((11, 12, false)));
        assert_eq!(literal_marker(b"A LOGIN {x}"), None);
        assert_eq!(literal_marker(b"A LOGIN {}"), None);
        assert_eq!(literal_marker(b"A NOOP"), None);
    }

    #[test]
    fn atoms_strings_and_lists() {
        let cmd = parse_command(vec![text(
            r#"a1 uid fetch 1:* (FLAGS BODY.PEEK[HEADER.FIELDS (From To)]<0.10>) "q \"x\" \\""#,
        )])
        .unwrap();
        assert_eq!(cmd.tag, "a1");
        assert_eq!(cmd.name, "UID FETCH");
        assert_eq!(
            cmd.args,
            vec![
                atom("1:*"),
                Value::List(vec![
                    atom("FLAGS"),
                    atom("BODY.PEEK[HEADER.FIELDS (From To)]<0.10>")
                ]),
                Value::Str(br#"q "x" \"#.to_vec()),
            ]
        );
    }

    #[test]
    fn literals_are_opaque() {
        let cmd = parse_command(vec![
            text("A APPEND INBOX (\\Seen) "),
            Part::Literal(b"a (b) \"c\r\n{3}\r\n".to_vec()),
            text(""),
        ])
        .unwrap();
        assert_eq!(
            cmd.args,
            vec![
                atom("INBOX"),
                Value::List(vec![atom("\\Seen")]),
                Value::Str(b"a (b) \"c\r\n{3}\r\n".to_vec()),
            ]
        );
    }

    #[test]
    fn literal_inside_a_list() {
        let cmd = parse_command(vec![
            text("A X ("),
            Part::Literal(b"one".to_vec()),
            text(" two)"),
        ])
        .unwrap();
        assert_eq!(
            cmd.args,
            vec![Value::List(vec![Value::Str(b"one".to_vec()), atom("two")])]
        );
    }

    #[test]
    fn errors() {
        assert!(parse_command(vec![text("A X (a")]).is_err());
        assert!(parse_command(vec![text("A X a)")]).is_err());
        assert!(parse_command(vec![text("A X \"open")]).is_err());
        assert!(parse_command(vec![text("A X BODY[HEADER")]).is_err());
        let (tag, _) = parse_command(vec![text("A7 X \"open")]).unwrap_err();
        assert_eq!(tag.as_deref(), Some("A7"));
        assert!(parse_command(vec![text("")]).is_err());
    }

    #[test]
    fn log_hides_literals_and_tag() {
        let parts = vec![
            text("A1 LOGIN "),
            Part::Literal(b"bob".to_vec()),
            text(" x"),
        ];
        assert_eq!(log_text(&parts), "LOGIN {3} x");
    }
}
