//! Reading the message that follows DATA, as strictly as real servers do.
//!
//! The message ends at CRLF `.` CRLF (RFC 5321 4.1.1.4) and is kept exactly
//! as sent, less the dot-stuffing (4.5.2): nothing is normalised. What a
//! strict server refuses, the fake refuses:
//!
//! - a bare LF or a CR not followed by LF (RFC 5321 2.3.8). Since then the
//!   end of the data is no longer certain (the "SMTP smuggling" problem),
//!   the server answers 5xx at once and closes the connection, as Postfix
//!   does with `smtpd_forbid_bare_newline`;
//! - a line longer than 1000 octets, CRLF included (4.5.3.1.6), answered
//!   with 5xx once the data has ended;
//! - a message over the size limit, answered with 552.

use std::io;

use crate::conn::Conn;

/// The longest text line RFC 5321 4.5.3.1.6 allows, CRLF included.
pub(crate) const MAX_TEXT_LINE: usize = 1000;

/// What reading the data gave.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Data {
    /// The message, dot-unstuffed, byte for byte as sent.
    Message(Vec<u8>),
    /// Refused with this reply; the session goes on.
    Refused(u16, &'static str),
    /// Refused with this reply, after which the connection closes.
    RefusedAndClosed(u16, &'static str),
    /// The client went away mid-message.
    Eof,
}

/// Reads the message after a 354 reply, keeping at most `max` bytes.
pub(super) async fn read(conn: &mut Conn, max: usize) -> io::Result<Data> {
    let mut data = Vec::new();
    let mut too_big = false;
    let mut too_long = false;
    loop {
        let Some(line) = conn.read_raw_line(MAX_TEXT_LINE + 1).await? else {
            return Ok(Data::Eof);
        };
        if !line.crlf || line.stray_cr {
            return Ok(Data::RefusedAndClosed(
                554,
                "5.6.0 Message contains a bare CR or LF; lines must end in CRLF (RFC 5321 2.3.8)",
            ));
        }
        // Every line so far ended in CRLF, so this one starts after one.
        if line.bytes == b".\r\n" {
            break;
        }
        if line.len > MAX_TEXT_LINE {
            too_long = true;
        }
        if too_long || too_big {
            continue;
        }
        let content = line.bytes.strip_prefix(b".").unwrap_or(&line.bytes);
        if data.len() + content.len() > max {
            too_big = true;
        } else {
            data.extend_from_slice(content);
        }
    }
    Ok(if too_long {
        Data::Refused(
            554,
            "5.6.0 Message has a line longer than 1000 octets (RFC 5321 4.5.3.1.6)",
        )
    } else if too_big {
        Data::Refused(552, "5.3.4 Message too big")
    } else {
        Data::Message(data)
    })
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    /// Reads `sent` as DATA would, with a size limit of `max`, and returns
    /// what came of it and the bytes left after it.
    async fn read_from(sent: &[u8], max: usize) -> (Data, Vec<u8>) {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        client.write_all(sent).await.unwrap();
        drop(client);
        let mut conn = Conn::new(Box::new(server), false);
        let data = read(&mut conn, max).await.unwrap();
        let mut rest = Vec::new();
        while let Some(line) = conn.read_raw_line(usize::MAX).await.unwrap() {
            rest.extend_from_slice(&line.bytes);
        }
        (data, rest)
    }

    #[tokio::test]
    async fn keeps_the_bytes_and_unstuffs_dots() {
        let (data, rest) = read_from(b"A: b\r\n\r\n..dot\r\n.x\r\n\r\n.\r\nQUIT\r\n", 1000).await;
        assert_eq!(
            data,
            Data::Message(b"A: b\r\n\r\n.dot\r\nx\r\n\r\n".to_vec())
        );
        assert_eq!(rest, b"QUIT\r\n");
        let (data, _) = read_from(b".\r\n", 1000).await;
        assert_eq!(data, Data::Message(Vec::new()));
    }

    #[tokio::test]
    async fn bare_lf_or_cr_is_refused_at_once() {
        for sent in [
            &b"one\ntwo\r\n.\r\n"[..],
            b"one\rtwo\r\n.\r\n",
            b"one\r\n.\n",
        ] {
            let (data, _) = read_from(sent, 1000).await;
            assert!(matches!(data, Data::RefusedAndClosed(554, _)), "{sent:?}");
        }
    }

    #[tokio::test]
    async fn long_lines_and_big_messages_are_refused_after_the_end() {
        // Exactly 1000 octets with the CRLF: allowed.
        let mut sent = vec![b'a'; MAX_TEXT_LINE - 2];
        sent.extend_from_slice(b"\r\n.\r\nNOOP\r\n");
        let (data, _) = read_from(&sent, 10_000).await;
        assert!(matches!(data, Data::Message(ref d) if d.len() == MAX_TEXT_LINE));

        let mut sent = vec![b'a'; MAX_TEXT_LINE - 1];
        sent.extend_from_slice(b"\r\n.\r\nNOOP\r\n");
        let (data, rest) = read_from(&sent, 10_000).await;
        assert!(matches!(data, Data::Refused(554, _)), "{data:?}");
        assert_eq!(rest, b"NOOP\r\n");

        let (data, rest) = read_from(b"12345\r\n.\r\nNOOP\r\n", 4).await;
        assert_eq!(data, Data::Refused(552, "5.3.4 Message too big"));
        assert_eq!(rest, b"NOOP\r\n");
    }
}
