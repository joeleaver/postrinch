//! Probing a guessed server before it is offered: a trait, so tests can
//! answer, and the real network prober.
//!
//! A TLS server (993, 465) must complete a TLS handshake that the
//! platform's verifier accepts, then greet as its protocol does. A STARTTLS
//! server (143, 587) must greet and offer STARTTLS. No one signs in.

use std::sync::Arc;

use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use rustls_platform_verifier::BuilderVerifierExt;
use thiserror::Error;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::BoxFuture;
use crate::types::Security;

/// The longest line read from a server while probing.
const MAX_LINE: usize = 8 * 1024;

/// The most lines read for one reply while probing.
const MAX_LINES: usize = 100;

/// Which protocol a server is probed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// Incoming mail.
    Imap,
    /// Outgoing mail.
    Smtp,
}

/// Why a probe failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProbeError {
    /// The TCP connection or the TLS handshake failed.
    #[error("could not connect: {0}")]
    Connect(String),
    /// The server did not greet as the protocol does.
    #[error("unexpected greeting: {0:?}")]
    Greeting(String),
    /// The server does not offer STARTTLS.
    #[error("the server does not offer STARTTLS")]
    NoStartTls,
    /// The server closed the connection or sent something unreadable.
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for ProbeError {
    fn from(e: std::io::Error) -> Self {
        ProbeError::Io(e.to_string())
    }
}

/// Checks that a server answers as `protocol` with `security`.
pub trait Probe: Send + Sync {
    /// Probes `host:port`. Timeouts are the caller's.
    fn probe<'a>(
        &'a self,
        protocol: Protocol,
        host: &'a str,
        port: u16,
        security: Security,
    ) -> BoxFuture<'a, Result<(), ProbeError>>;
}

/// The real prober, verifying certificates with the platform's verifier.
#[derive(Clone)]
pub struct NetProbe {
    tls: TlsConnector,
}

impl std::fmt::Debug for NetProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NetProbe")
    }
}

impl NetProbe {
    /// A prober using the platform's certificate verifier and aws-lc-rs.
    pub fn new() -> Result<Self, rustls::Error> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_platform_verifier()?
            .with_no_client_auth();
        Ok(NetProbe {
            tls: TlsConnector::from(Arc::new(config)),
        })
    }

    async fn run(
        &self,
        protocol: Protocol,
        host: &str,
        port: u16,
        security: Security,
    ) -> Result<(), ProbeError> {
        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|e| ProbeError::Connect(e.to_string()))?;
        match security {
            Security::Tls => {
                let name = ServerName::try_from(host.to_owned())
                    .map_err(|e| ProbeError::Connect(e.to_string()))?;
                let tls = self
                    .tls
                    .connect(name, tcp)
                    .await
                    .map_err(|e| ProbeError::Connect(e.to_string()))?;
                let mut stream = BufReader::new(tls);
                match protocol {
                    Protocol::Imap => imap_greeting(&mut stream).await.map(drop),
                    Protocol::Smtp => smtp_reply(&mut stream, 220).await.map(drop),
                }
            }
            Security::StartTls => {
                let local = tcp.local_addr()?.ip();
                let mut stream = BufReader::new(tcp);
                match protocol {
                    Protocol::Imap => imap_starttls(&mut stream).await,
                    Protocol::Smtp => smtp_starttls(&mut stream, local).await,
                }
            }
        }
    }
}

impl Probe for NetProbe {
    fn probe<'a>(
        &'a self,
        protocol: Protocol,
        host: &'a str,
        port: u16,
        security: Security,
    ) -> BoxFuture<'a, Result<(), ProbeError>> {
        Box::pin(self.run(protocol, host, port, security))
    }
}

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
async fn imap_greeting<S: AsyncBufRead + Unpin>(stream: &mut S) -> Result<String, ProbeError> {
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

/// An IMAP server on a plain port: greets, and offers STARTTLS in its
/// greeting's capabilities or in answer to CAPABILITY.
async fn imap_starttls<S>(stream: &mut BufReader<S>) -> Result<(), ProbeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let greeting = imap_greeting(stream).await?;
    let offered = if greeting.to_ascii_uppercase().contains("[CAPABILITY ") {
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
    // Courtesy only: the probe's answer is already known.
    let _ = stream.get_mut().write_all(b"P2 LOGOUT\r\n").await;
    if offered {
        Ok(())
    } else {
        Err(ProbeError::NoStartTls)
    }
}

/// Reads an SMTP reply (all its lines), which must have status `code`.
async fn smtp_reply<S: AsyncBufRead + Unpin>(
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

/// An SMTP server on a plain port: greets with 220 and lists STARTTLS in
/// its answer to EHLO.
async fn smtp_starttls<S>(
    stream: &mut BufReader<S>,
    local: std::net::IpAddr,
) -> Result<(), ProbeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    smtp_reply(stream, 220).await?;
    // RFC 5321: a client without a domain name gives its address literal.
    let ehlo = match local {
        std::net::IpAddr::V4(ip) => format!("EHLO [{ip}]\r\n"),
        std::net::IpAddr::V6(ip) => format!("EHLO [IPv6:{ip}]\r\n"),
    };
    stream.get_mut().write_all(ehlo.as_bytes()).await?;
    let lines = smtp_reply(stream, 250).await?;
    // Courtesy only: the probe's answer is already known.
    let _ = stream.get_mut().write_all(b"QUIT\r\n").await;
    let offered = lines.iter().skip(1).any(|line| {
        line.get(4..)
            .is_some_and(|keyword| keyword.trim().eq_ignore_ascii_case("STARTTLS"))
    });
    if offered {
        Ok(())
    } else {
        Err(ProbeError::NoStartTls)
    }
}
