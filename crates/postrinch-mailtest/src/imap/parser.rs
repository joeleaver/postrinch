//! Splitting IMAP commands into values.
//!
//! `reader` gathers a command off the wire as [`Part`]s (text, and the
//! literals between, never looked inside), and [`tokenize`] turns them
//! into [`Value`]s:
//!
//! - atoms (`FETCH`, `\Seen`, `1:*`, `BODY.PEEK[HEADER.FIELDS (From)]<0.10>`:
//!   a `[` inside an atom runs to its matching `]`, spaces and parentheses
//!   included, as RFC 3501's fetch attributes need);
//! - strings, quoted (with `\"` and `\\` escapes) or literal;
//! - parenthesized lists of values.

use std::fmt;

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

/// A syntax error in a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyntaxError(pub(crate) String);

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The first space-separated word of the command, used as the tag of a
/// BAD answer to a line that does not parse.
pub(crate) fn first_word(parts: &[Part]) -> Option<String> {
    let Some(Part::Text(text)) = parts.first() else {
        return None;
    };
    let word = text.split(|&b| b == b' ').next()?;
    let word = std::str::from_utf8(word).ok()?;
    (!word.is_empty() && word.bytes().all(is_tag_char)).then(|| word.to_string())
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
}
