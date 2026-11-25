//! Folder names as commands send them back to the server.
//!
//! Names from LIST are mostly 7-bit (modified UTF-7) and go as quoted
//! strings. Some servers list raw 8-bit names instead; those, and any
//! name with a character a quoted string cannot hold, go as literals,
//! which RFC 3501 allows for every mailbox argument. A name the client no
//! longer has exactly cannot be sent at all: imap-proto reads names as
//! UTF-8 and replaces bytes that are not with U+FFFD, so such a name (or
//! one with a NUL, which no string may hold) is skipped with a warning.

/// One piece of a command: text as it is, or bytes sent as a literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Piece {
    Text(String),
    Literal(Vec<u8>),
}

/// A command to send, in pieces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Command(Vec<Piece>);

/// A command as it goes on the wire: its first line, which is text and
/// goes with the command's tag, then the lines after each literal. Every
/// line is followed by CRLF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Wire {
    pub first: String,
    pub rest: Vec<Line>,
}

/// A line after a literal's `{n}`: the literal's bytes and the text up to
/// the next literal's `{n}`, or the command's end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Line {
    pub bytes: Vec<u8>,
    /// Whether it waits for the server's `+` (a synchronizing literal).
    pub wait: bool,
}

impl Command {
    /// A command of plain text.
    pub(super) fn plain(text: impl Into<String>) -> Self {
        Self(vec![Piece::Text(text.into())])
    }

    /// `verb` and a folder name; `None` when the name cannot be sent.
    pub(super) fn with_mailbox(verb: &str, name: &str) -> Option<Self> {
        let mut pieces = vec![Piece::Text(format!("{verb} "))];
        pieces.push(mailbox(name)?);
        Some(Self(pieces))
    }

    /// Adds text at the end.
    pub(super) fn then(mut self, text: &str) -> Self {
        self.0.push(Piece::Text(text.to_owned()));
        self
    }

    /// Adds a literal at the end, as APPEND's message.
    pub(super) fn literal(mut self, bytes: Vec<u8>) -> Self {
        self.0.push(Piece::Literal(bytes));
        self
    }

    /// The command's first words, to name it in errors without its
    /// arguments.
    pub(super) fn verb(&self) -> &str {
        let first = match self.0.first() {
            Some(Piece::Text(text)) => text.as_str(),
            _ => "",
        };
        let mut words = first.split(' ');
        match (words.next(), words.next()) {
            (Some("UID"), Some(second)) => &first[..4 + second.len()],
            (Some(word), _) => word,
            _ => first,
        }
    }

    /// The command as lines to send. With `non_synchronizing` (LITERAL+,
    /// or LITERAL- for a literal of at most 4096 bytes: RFC 7888) each
    /// literal is `{n+}` and its line goes at once; otherwise it is `{n}`
    /// and its line waits for the server's `+`.
    pub(super) fn wire(&self, literal_plus: bool, literal_minus: bool) -> Wire {
        let mut first = String::new();
        let mut rest: Vec<Line> = Vec::new();
        for piece in &self.0 {
            match piece {
                Piece::Text(text) => match rest.last_mut() {
                    Some(line) => line.bytes.extend_from_slice(text.as_bytes()),
                    None => first.push_str(text),
                },
                Piece::Literal(bytes) => {
                    let n = bytes.len();
                    let plus = literal_plus || (literal_minus && n <= 4096);
                    let marker = if plus {
                        format!("{{{n}+}}")
                    } else {
                        format!("{{{n}}}")
                    };
                    match rest.last_mut() {
                        Some(line) => line.bytes.extend_from_slice(marker.as_bytes()),
                        None => first.push_str(&marker),
                    }
                    rest.push(Line {
                        bytes: bytes.clone(),
                        wait: !plus,
                    });
                }
            }
        }
        Wire { first, rest }
    }
}

/// A folder name as a command argument.
fn mailbox(name: &str) -> Option<Piece> {
    if name.contains(['\0', char::REPLACEMENT_CHARACTER]) {
        return None;
    }
    if name.is_ascii() && !name.contains(['\r', '\n']) {
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        return Some(Piece::Text(format!("\"{escaped}\"")));
    }
    Some(Piece::Literal(name.as_bytes().to_vec()))
}

/// Whether a folder's name can be sent back to the server.
pub(super) fn sendable(name: &str) -> bool {
    mailbox(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_names_are_quoted() {
        let c = Command::with_mailbox("SELECT", "Work \"x\"").unwrap();
        assert_eq!(c.wire(false, false).first, "SELECT \"Work \\\"x\\\"\"");
        assert!(c.wire(false, false).rest.is_empty());
    }

    #[test]
    fn eight_bit_names_are_literals() {
        let c = Command::with_mailbox("SELECT", "Entwürfe")
            .unwrap()
            .then(" (CONDSTORE)");
        let line = |wait| Line {
            bytes: "Entwürfe (CONDSTORE)".as_bytes().to_vec(),
            wait,
        };
        let plus = c.wire(true, false);
        assert_eq!(
            (plus.first.as_str(), plus.rest),
            ("SELECT {9+}", vec![line(false)])
        );
        assert_eq!(c.wire(false, true), c.wire(true, false));
        let waiting = c.wire(false, false);
        assert_eq!(
            (waiting.first.as_str(), waiting.rest),
            ("SELECT {9}", vec![line(true)])
        );
    }

    #[test]
    fn literals_are_bytes() {
        let message = b"Subject: \xff\r\n\r\nHi\r\n".to_vec();
        let c = Command::with_mailbox("APPEND", "Sent")
            .unwrap()
            .then(" (\\Seen) ")
            .literal(message.clone());
        let wire = c.wire(false, true);
        assert_eq!(
            wire.first,
            format!("APPEND \"Sent\" (\\Seen) {{{}+}}", message.len())
        );
        assert_eq!(
            wire.rest,
            [Line {
                bytes: message,
                wait: false
            }]
        );
        let big = Command::plain("APPEND x ").literal(vec![b'a'; 5000]);
        assert_eq!(big.wire(false, true).first, "APPEND x {5000}");
    }

    #[test]
    fn names_that_were_not_utf8_cannot_be_sent() {
        assert!(!sendable("Entw\u{FFFD}rfe"));
        assert!(!sendable("a\0b"));
        assert!(sendable("Entwürfe"));
    }

    #[test]
    fn verbs_name_commands_without_arguments() {
        assert_eq!(Command::plain("UID FETCH 1:* (FLAGS)").verb(), "UID FETCH");
        assert_eq!(
            Command::with_mailbox("SELECT", "INBOX").unwrap().verb(),
            "SELECT"
        );
        assert_eq!(Command::plain("NOOP").verb(), "NOOP");
    }
}
