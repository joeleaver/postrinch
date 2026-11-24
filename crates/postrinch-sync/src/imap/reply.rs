//! A server's tagged answer, and the stream that keeps it for sign-in.
//!
//! When a sign-in is refused, async-imap keeps only a debug rendering of
//! the server's answer. The person is owed the server's own sentence
//! ("Invalid credentials", "Application-specific password required"), so
//! the stream under async-imap ([`ImapStream`]) can record what it reads
//! while a sign-in is under way, and [`Tagged::last_in`] finds the answer
//! in it.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    No,
    Bad,
}

/// A tagged answer: its status, its response code's name (upper-cased,
/// as `AUTHENTICATIONFAILED`) and the server's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tagged {
    pub(crate) status: Status,
    pub(crate) code: Option<String>,
    pub(crate) text: String,
}

impl Tagged {
    /// Parses what follows the tag: `NO [AUTHENTICATIONFAILED] text`.
    /// Anything but OK or NO counts as BAD.
    pub(crate) fn parse(rest: &str) -> Self {
        let (status, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        let status = match status.to_ascii_uppercase().as_str() {
            "OK" => Status::Ok,
            "NO" => Status::No,
            _ => Status::Bad,
        };
        Self {
            status,
            ..Self::parse_text(rest)
        }
    }

    /// Parses `[CODE args] text` (as for a status of OK).
    pub(crate) fn parse_text(rest: &str) -> Self {
        let rest = rest.trim();
        let (code, text) = match rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            Some((code, text)) => {
                let name = code.split(' ').next().unwrap_or_default();
                (Some(name.to_ascii_uppercase()), text.trim())
            }
            None => (None, rest),
        };
        Self {
            status: Status::Ok,
            code,
            text: text.to_owned(),
        }
    }

    /// The last tagged answer in `bytes`, what a server sent.
    pub(crate) fn last_in(bytes: &[u8]) -> Option<Self> {
        String::from_utf8_lossy(bytes)
            .split("\r\n")
            .filter(|line| !line.is_empty() && !line.starts_with("* ") && !line.starts_with('+'))
            .filter_map(|line| line.split_once(' '))
            .map(|(_, rest)| Self::parse(rest))
            .last()
    }
}

/// The most a recording keeps: a sign-in's answers are a few lines.
const MAX_RECORDED: usize = 16 * 1024;

#[derive(Debug, Default)]
struct Recording {
    on: AtomicBool,
    bytes: Mutex<Vec<u8>>,
}

/// The stream async-imap speaks over: TLS on TCP, recording what it reads
/// while asked to.
pub struct ImapStream {
    inner: TlsStream<TcpStream>,
    recording: Arc<Recording>,
}

impl std::fmt::Debug for ImapStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapStream").finish_non_exhaustive()
    }
}

/// The handle that starts and stops an [`ImapStream`]'s recording.
#[derive(Clone)]
pub(crate) struct Recorder(Arc<Recording>);

impl ImapStream {
    pub(crate) fn new(inner: TlsStream<TcpStream>) -> (Self, Recorder) {
        let recording = Arc::new(Recording::default());
        let recorder = Recorder(recording.clone());
        (Self { inner, recording }, recorder)
    }
}

impl Recorder {
    /// Starts recording afresh.
    pub(crate) fn start(&self) {
        self.0
            .bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.0.on.store(true, Ordering::Release);
    }

    /// Stops recording; what was read meanwhile.
    pub(crate) fn stop(&self) -> Vec<u8> {
        self.0.on.store(false, Ordering::Release);
        std::mem::take(&mut *self.0.bytes.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl AsyncRead for ImapStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(polled, Poll::Ready(Ok(()))) && self.recording.on.load(Ordering::Acquire) {
            let mut bytes = self
                .recording
                .bytes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            bytes.extend_from_slice(&buf.filled()[before..]);
            if bytes.len() > MAX_RECORDED {
                let excess = bytes.len() - MAX_RECORDED;
                bytes.drain(..excess);
            }
        }
        polled
    }
}

impl AsyncWrite for ImapStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_code_and_text() {
        assert_eq!(
            Tagged::parse("NO [AUTHENTICATIONFAILED] Invalid credentials"),
            Tagged {
                status: Status::No,
                code: Some("AUTHENTICATIONFAILED".into()),
                text: "Invalid credentials".into(),
            }
        );
        let ok = Tagged::parse("ok [CAPABILITY IMAP4rev1 IDLE] Logged in");
        assert_eq!(ok.status, Status::Ok);
        assert_eq!(ok.code.as_deref(), Some("CAPABILITY"));
        assert_eq!(Tagged::parse("BAD what?").text, "what?");
    }

    #[test]
    fn finds_the_last_tagged_answer() {
        let bytes = b"+ eyJzdGF0dXMiOiI0MDEifQ==\r\n* OK still here\r\na3 NO [AUTHENTICATIONFAILED] Invalid credentials\r\n";
        let tagged = Tagged::last_in(bytes).unwrap();
        assert_eq!(tagged.status, Status::No);
        assert_eq!(tagged.text, "Invalid credentials");
        assert!(Tagged::last_in(b"* OK hi\r\n").is_none());
    }
}
