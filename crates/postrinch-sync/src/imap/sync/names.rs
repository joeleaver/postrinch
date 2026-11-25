//! Folder names as commands send them back to the server.
//!
//! Names from LIST are mostly 7-bit (modified UTF-7) and go as quoted
//! strings. Some servers list raw 8-bit names instead; those, and any
//! name with a character a quoted string cannot hold, go as literals,
//! which RFC 3501 allows for every mailbox argument. A name the client no
//! longer has exactly cannot be sent at all: imap-proto reads names as
//! UTF-8 and replaces bytes that are not with U+FFFD, so such a name (or
//! one with a NUL, which no string may hold) is skipped with a warning.

/// One piece of a command: text as it is, or a string sent as a literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Piece {
    Text(String),
    Literal(String),
}

/// A command to send, in pieces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Command(Vec<Piece>);

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
    /// literal is sent at once as `{n+}`, and the command is one line;
    /// otherwise each `{n}` ends a line and the server's `+` is awaited
    /// before the next.
    pub(super) fn lines(&self, literal_plus: bool, literal_minus: bool) -> Vec<String> {
        let mut lines = vec![String::new()];
        for piece in &self.0 {
            let line = lines.last_mut().expect("never empty");
            match piece {
                Piece::Text(text) => line.push_str(text),
                Piece::Literal(text) => {
                    let n = text.len();
                    if literal_plus || (literal_minus && n <= 4096) {
                        line.push_str(&format!("{{{n}+}}\r\n{text}"));
                    } else {
                        line.push_str(&format!("{{{n}}}"));
                        lines.push(text.clone());
                    }
                }
            }
        }
        lines
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
    Some(Piece::Literal(name.to_owned()))
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
        assert_eq!(c.lines(false, false), ["SELECT \"Work \\\"x\\\"\""]);
    }

    #[test]
    fn eight_bit_names_are_literals() {
        let c = Command::with_mailbox("SELECT", "Entwürfe")
            .unwrap()
            .then(" (CONDSTORE)");
        assert_eq!(
            c.lines(true, false),
            ["SELECT {9+}\r\nEntwürfe (CONDSTORE)"]
        );
        assert_eq!(c.lines(false, true), c.lines(true, false));
        assert_eq!(
            c.lines(false, false),
            ["SELECT {9}", "Entwürfe (CONDSTORE)"]
        );
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
