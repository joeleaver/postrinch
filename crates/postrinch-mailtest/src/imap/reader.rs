//! Reading IMAP commands off the wire.
//!
//! A command is one line, except that it may carry literals: a line ending
//! in `{n}` (synchronizing: the server answers `+` before the client sends
//! the bytes) or `{n+}` (LITERAL+, RFC 7888: the bytes follow at once) is
//! followed by exactly `n` bytes of anything, CRLF included, and then the
//! rest of the command on a new line. [`read_command`] gathers those pieces
//! as [`Part`]s without ever looking inside a literal; `parser` splits
//! them into values.

use std::io;

use super::parser::{Command, Part, SyntaxError, first_word, parse_command};
use crate::conn::Conn;

/// The largest literal the fake accepts (a message to APPEND, say).
pub(crate) const MAX_LITERAL: usize = 64 * 1024 * 1024;

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
                    return refuse_command(conn, tag, size, sync).await;
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

/// Refuses a command whose literal of `size` bytes is too large, after
/// reading past the rest of it so none of its bytes are taken for the next
/// command.
///
/// The client sends a synchronizing literal only after a `+`, so refusing
/// one at once is enough: the client gives the command up. A
/// non-synchronizing literal (LITERAL+) is already on its way, so its bytes
/// and the rest of the command, including any further non-synchronizing
/// literals, are read and thrown away first (RFC 7888 section 4, the first
/// of its two choices). A further synchronizing literal ends the command
/// there, as the client again waits for a `+` that does not come.
async fn refuse_command(
    conn: &mut Conn,
    tag: Option<String>,
    size: usize,
    sync: bool,
) -> io::Result<Read> {
    let invalid = |tag| {
        Ok(Read::Invalid {
            tag,
            reason: "[TOOBIG] Literal too large".into(),
        })
    };
    if sync {
        return invalid(tag);
    }
    let mut size = size;
    loop {
        if !conn.discard(size).await? {
            return Ok(Read::Eof);
        }
        let Some(line) = conn.read_line().await? else {
            return Ok(Read::Eof);
        };
        match literal_marker(&line) {
            Some((_, next, false)) => size = next,
            Some((_, _, true)) | None => return invalid(tag),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Part {
        Part::Text(s.as_bytes().to_vec())
    }

    #[test]
    fn markers() {
        assert_eq!(literal_marker(b"A APPEND x {12}"), Some((11, 12, true)));
        assert_eq!(literal_marker(b"A APPEND x {12+}"), Some((11, 12, false)));
        assert_eq!(literal_marker(b"A LOGIN {x}"), None);
        assert_eq!(literal_marker(b"A LOGIN {}"), None);
        assert_eq!(literal_marker(b"A NOOP"), None);
    }

    /// Refuses a command whose first literal (`size` bytes) is too large,
    /// with `sent` arriving after its marker line; returns the reply and
    /// what is left for the next command.
    async fn refuse(sent: &[u8], size: usize, sync: bool) -> (Read, Vec<u8>) {
        use tokio::io::AsyncWriteExt;
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        client.write_all(sent).await.unwrap();
        drop(client);
        let mut conn = Conn::new(Box::new(server), false);
        let read = refuse_command(&mut conn, Some("a1".into()), size, sync)
            .await
            .unwrap();
        let mut rest = Vec::new();
        while let Some(line) = conn.read_line().await.unwrap() {
            rest.extend_from_slice(&line);
            rest.push(b'\n');
        }
        (read, rest)
    }

    #[tokio::test]
    async fn an_oversized_literal_is_drained_before_bad() {
        // The literal looks like commands; none of it may be run.
        let (read, rest) = refuse(b"b LOGOUT\r\n {3+}\r\nabc x\r\nb2 NOOP\r\n", 10, false).await;
        assert!(
            matches!(read, Read::Invalid { ref tag, ref reason }
                if tag.as_deref() == Some("a1") && reason.starts_with("[TOOBIG]")),
            "{read:?}"
        );
        assert_eq!(rest, b"b2 NOOP\n");

        // A synchronizing literal was never sent: nothing to drain.
        let (_, rest) = refuse(b"b2 NOOP\r\n", 10, true).await;
        assert_eq!(rest, b"b2 NOOP\n");

        // A later synchronizing literal ends the command at its marker.
        let (_, rest) = refuse(b"0123456789 {4}\r\nb2 NOOP\r\n", 10, false).await;
        assert_eq!(rest, b"b2 NOOP\n");

        // The client went away mid-literal.
        let (read, _) = refuse(b"short", 10, false).await;
        assert!(matches!(read, Read::Eof));
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
