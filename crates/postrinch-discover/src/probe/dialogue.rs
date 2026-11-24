//! What the prober says to a server and what it expects back: greetings,
//! and the plain-text part of STARTTLS up to the server's go-ahead.

use std::net::IpAddr;

use tokio::io::BufReader;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};

use super::ProbeError;

/// The longest line read from a server while probing.
const MAX_LINE: usize = 8 * 1024;

/// The most lines read for one reply while probing.
const MAX_LINES: usize = 100;

/// Reads one line, without its line ending.
async fn read_line<S: AsyncBufRead + Unpin>(stream: &mut S) -> Result<String, ProbeError> {
    let mut line = Vec::new();
    let read = (&mut *stream)
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut line)
        .await?;
    if read == 0 {
        return Err(ProbeError::Io(
            "the server closed the connection".to_owned(),
        ));
    }
    if line.last() != Some(&b'\n') {
        return Err(ProbeError::Io(
            "the server sent an over-long line".to_owned(),
        ));
    }
    let text = String::from_utf8_lossy(&line);
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

/// Reads an IMAP greeting, which must be `* OK` or `* PREAUTH`.
pub(super) async fn imap_greeting<S: AsyncBufRead + Unpin>(
    stream: &mut S,
) -> Result<String, ProbeError> {
    let line = read_line(stream).await?;
    let upper = line.to_ascii_uppercase();
    if upper.starts_with("* OK") || upper.starts_with("* PREAUTH") {
        Ok(line)
    } else {
        Err(ProbeError::Greeting(line))
    }
}

/// `text` lists `STARTTLS` among its space-separated words.
fn lists_starttls(text: &str) -> bool {
    text.split([' ', '[', ']'])
        .any(|word| word.eq_ignore_ascii_case("STARTTLS"))
}

/// An IMAP server on a plain port, up to TLS: it greets with `* OK` (a
/// `* PREAUTH` greeting would sign in before TLS, RFC 9051 §7.1.4), offers
/// STARTTLS in its greeting's capabilities or in answer to CAPABILITY, and
/// answers `OK` to STARTTLS. The stream is then ready for the handshake.
pub(super) async fn imap_starttls<S>(stream: &mut BufReader<S>) -> Result<(), ProbeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let greeting = imap_greeting(stream).await?;
    let upper = greeting.to_ascii_uppercase();
    if upper.starts_with("* PREAUTH") {
        return Err(ProbeError::PreAuth);
    }
    let offered = if upper.contains("[CAPABILITY ") {
        lists_starttls(&greeting)
    } else {
        stream.get_mut().write_all(b"P1 CAPABILITY\r\n").await?;
        let mut offered = false;
        for _ in 0..MAX_LINES {
            let line = read_line(stream).await?;
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("* CAPABILITY ") {
                offered |= lists_starttls(&line);
            } else if upper.starts_with("P1 ") {
                break;
            }
        }
        offered
    };
    if !offered {
        // Courtesy only: the probe's answer is already known.
        let _ = stream.get_mut().write_all(b"P2 LOGOUT\r\n").await;
        return Err(ProbeError::NoStartTls);
    }
    stream.get_mut().write_all(b"P2 STARTTLS\r\n").await?;
    for _ in 0..MAX_LINES {
        let line = read_line(stream).await?;
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("P2 OK") {
            return Ok(());
        }
        if upper.starts_with("P2 ") {
            return Err(ProbeError::StartTlsRefused(line));
        }
    }
    Err(ProbeError::Io("the server's reply did not end".to_owned()))
}

/// Reads an SMTP reply (all its lines), which must have status `code`.
pub(super) async fn smtp_reply<S: AsyncBufRead + Unpin>(
    stream: &mut S,
    code: u16,
) -> Result<Vec<String>, ProbeError> {
    let mut lines = Vec::new();
    for _ in 0..MAX_LINES {
        let line = read_line(stream).await?;
        let status = line.get(..3).and_then(|s| s.parse::<u16>().ok());
        if status != Some(code) {
            return Err(ProbeError::Greeting(line));
        }
        let last = line.as_bytes().get(3) != Some(&b'-');
        lines.push(line);
        if last {
            return Ok(lines);
        }
    }
    Err(ProbeError::Io("the server's reply did not end".to_owned()))
}

/// An SMTP server on a plain port, up to TLS: it greets with 220, lists
/// STARTTLS in its answer to EHLO, and answers 220 to STARTTLS. The stream
/// is then ready for the handshake.
pub(super) async fn smtp_starttls<S>(
    stream: &mut BufReader<S>,
    local: IpAddr,
) -> Result<(), ProbeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    smtp_reply(stream, 220).await?;
    // RFC 5321: a client without a domain name gives its address literal.
    let ehlo = match local {
        IpAddr::V4(ip) => format!("EHLO [{ip}]\r\n"),
        IpAddr::V6(ip) => format!("EHLO [IPv6:{ip}]\r\n"),
    };
    stream.get_mut().write_all(ehlo.as_bytes()).await?;
    let lines = smtp_reply(stream, 250).await?;
    let offered = lines.iter().skip(1).any(|line| {
        line.get(4..)
            .is_some_and(|keyword| keyword.trim().eq_ignore_ascii_case("STARTTLS"))
    });
    if !offered {
        // Courtesy only: the probe's answer is already known.
        let _ = stream.get_mut().write_all(b"QUIT\r\n").await;
        return Err(ProbeError::NoStartTls);
    }
    stream.get_mut().write_all(b"STARTTLS\r\n").await?;
    match smtp_reply(stream, 220).await {
        Ok(_) => Ok(()),
        Err(ProbeError::Greeting(line)) => Err(ProbeError::StartTlsRefused(line)),
        Err(e) => Err(e),
    }
}
