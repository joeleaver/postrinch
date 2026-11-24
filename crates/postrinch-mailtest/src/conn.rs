//! A buffered, line-oriented connection shared by the IMAP and SMTP fakes.
//!
//! Both protocols read CRLF-terminated lines with the occasional counted
//! block of bytes (an IMAP literal) in between, and both can switch the
//! same socket to TLS halfway through (STARTTLS). [`Conn`] owns the socket
//! and its own read buffer, so:
//!
//! - [`Conn::fill`] is cancel-safe: a session can wait in `select!` for
//!   either more input or a change to the mailbox (IDLE) without losing
//!   bytes that arrived while it was waiting;
//! - [`Conn::start_tls`] can check that the client did not pipeline
//!   anything after its STARTTLS command (which would let a man in the
//!   middle inject plain-text commands into the protected session).

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::TlsAcceptor;

/// The longest line either fake accepts, so a broken client cannot make a
/// test eat memory without end.
pub(crate) const MAX_LINE: usize = 64 * 1024;

/// Anything a session can talk over: a TCP socket or a TLS stream on one.
pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// One client connection with its read buffer.
pub(crate) struct Conn {
    stream: Option<Box<dyn Io>>,
    buf: Vec<u8>,
    tls: bool,
}

impl Conn {
    /// Wraps an accepted stream; `tls` says whether it is already encrypted.
    pub(crate) fn new(stream: Box<dyn Io>, tls: bool) -> Self {
        Self {
            stream: Some(stream),
            buf: Vec::new(),
            tls,
        }
    }

    /// Whether the connection is encrypted (implicit TLS or after STARTTLS).
    pub(crate) fn is_tls(&self) -> bool {
        self.tls
    }

    fn stream(&mut self) -> io::Result<&mut Box<dyn Io>> {
        self.stream
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "connection closed"))
    }

    /// Reads whatever the peer has sent next into the buffer. Returns
    /// `false` at end of stream. Cancel-safe: if the future is dropped
    /// before it completes, no input is lost.
    pub(crate) async fn fill(&mut self) -> io::Result<bool> {
        let mut chunk = [0u8; 8192];
        let n = self.stream()?.read(&mut chunk).await?;
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(n > 0)
    }

    /// Whether a whole line is already buffered.
    pub(crate) fn has_line(&self) -> bool {
        self.buf.contains(&b'\n')
    }

    /// Takes one buffered line, without its line ending, if a whole one
    /// has arrived. A bare LF is accepted as a line ending as well as CRLF.
    pub(crate) fn take_line(&mut self) -> Option<Vec<u8>> {
        let end = self.buf.iter().position(|&b| b == b'\n')?;
        let mut line: Vec<u8> = self.buf.drain(..=end).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Some(line)
    }

    /// Reads one line, waiting for it as needed. `None` at end of stream.
    pub(crate) async fn read_line(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(line) = self.take_line() {
                return Ok(Some(line));
            }
            if self.buf.len() > MAX_LINE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "line longer than the fake server accepts",
                ));
            }
            if !self.fill().await? {
                return Ok(None);
            }
        }
    }

    /// Reads exactly `n` bytes, waiting for them as needed. `None` at end
    /// of stream.
    pub(crate) async fn read_exact(&mut self, n: usize) -> io::Result<Option<Vec<u8>>> {
        while self.buf.len() < n {
            if !self.fill().await? {
                return Ok(None);
            }
        }
        Ok(Some(self.buf.drain(..n).collect()))
    }

    /// Writes and flushes `bytes`.
    pub(crate) async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let stream = self.stream()?;
        stream.write_all(bytes).await?;
        stream.flush().await
    }

    /// Switches the connection to TLS after a STARTTLS exchange. Fails if
    /// the client sent anything after its STARTTLS command, since those
    /// bytes were not protected (RFC 3207 section 4.2, RFC 9051 6.2.1).
    pub(crate) async fn start_tls(&mut self, acceptor: &TlsAcceptor) -> io::Result<()> {
        if !self.buf.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "client pipelined data after STARTTLS",
            ));
        }
        let plain = self
            .stream
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "connection closed"))?;
        let tls = acceptor.accept(plain).await?;
        self.stream = Some(Box::new(tls));
        self.tls = true;
        Ok(())
    }

    /// Closes the connection without saying anything more.
    pub(crate) async fn close(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            // The peer may already be gone; a failed shutdown changes nothing.
            let _ = stream.shutdown().await;
        }
    }
}
