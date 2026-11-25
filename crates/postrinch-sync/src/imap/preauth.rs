//! The IMAP exchange before signing in: the greeting, CAPABILITY and
//! STARTTLS, spoken line by line on the raw stream before async-imap takes
//! it over.
//!
//! Done here rather than through async-imap so that nothing the server
//! sent before TLS can be read as if it came after: the buffer must be
//! empty when the stream changes hands (RFC 8314's STARTTLS injection).

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use super::capabilities::Capabilities;
use super::refusal::{is_busy, names_connections};
use super::reply::{Status, Tagged};
use crate::{SyncError, SyncResult};

/// The longest line accepted before signing in.
const MAX_LINE: u64 = 64 * 1024;

/// A stream being spoken to before sign-in.
pub(crate) struct PreAuth<S> {
    reader: BufReader<S>,
    server: String,
    next_tag: u32,
}

impl<S: AsyncRead + AsyncWrite + Unpin> PreAuth<S> {
    pub(crate) fn new(stream: S, server: &str) -> Self {
        Self {
            reader: BufReader::new(stream),
            server: server.to_owned(),
            next_tag: 1,
        }
    }

    fn error(&self, message: impl Into<String>) -> SyncError {
        SyncError::Server {
            service: "IMAP",
            server: self.server.clone(),
            message: message.into(),
        }
    }

    /// What a `BYE` before sign-in means: the account turned away for
    /// now ([`SyncError::Busy`], [`is_busy`]), or the server failing.
    fn bye(&self, rest: &str) -> SyncError {
        let bye = Tagged::parse_text(rest);
        if is_busy(bye.code.as_deref(), &bye.text) {
            return SyncError::Busy {
                service: "IMAP",
                server: self.server.clone(),
                connections: names_connections(bye.code.as_deref(), &bye.text),
                message: bye.text,
            };
        }
        self.error(bye.text)
    }

    async fn read_line(&mut self) -> SyncResult<String> {
        let mut line = Vec::new();
        (&mut self.reader)
            .take(MAX_LINE)
            .read_until(b'\n', &mut line)
            .await?;
        if line.is_empty() {
            return Err(self.error("the server closed the connection"));
        }
        if !line.ends_with(b"\n") {
            return Err(self.error("the server sent a line too long to be IMAP"));
        }
        let text = String::from_utf8_lossy(&line);
        Ok(text.trim_end_matches(['\r', '\n']).to_owned())
    }

    /// Reads the greeting. Its capabilities, if it lists them.
    pub(crate) async fn greeting(&mut self) -> SyncResult<Option<Capabilities>> {
        let line = self.read_line().await?;
        let Some(rest) = line.strip_prefix("* ") else {
            return Err(self.error(format!("the server greeted with {line:?}")));
        };
        let (status, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        match status.to_ascii_uppercase().as_str() {
            "OK" => Ok(capability_code(rest)),
            // Signed in by something outside IMAP: there is no sign-in to
            // do, which async-imap cannot follow, and Postrinch would not
            // know whose mail it was.
            "PREAUTH" => Err(self.error("the server signed in on its own, before being asked")),
            // RFC 9051 7.1.5: the server will not serve this connection.
            "BYE" => Err(self.bye(rest)),
            _ => Err(self.error(format!("the server greeted with {line:?}"))),
        }
    }

    /// Sends a command and reads to its tagged answer. The untagged lines
    /// before it, and the answer.
    async fn command(&mut self, command: &str) -> SyncResult<(Vec<String>, Tagged)> {
        let tag = format!("p{}", self.next_tag);
        self.next_tag += 1;
        let stream = self.reader.get_mut();
        stream
            .write_all(format!("{tag} {command}\r\n").as_bytes())
            .await?;
        stream.flush().await?;
        let mut untagged = Vec::new();
        loop {
            let line = self.read_line().await?;
            if let Some(rest) = line.strip_prefix(&format!("{tag} ")) {
                return Ok((untagged, Tagged::parse(rest)));
            }
            if let Some(rest) = line.strip_prefix("* ") {
                if let Some(text) = rest.strip_prefix("BYE ") {
                    return Err(self.bye(text));
                }
                untagged.push(rest.to_owned());
            } else {
                return Err(self.error(format!("the server answered {line:?}")));
            }
        }
    }

    /// Asks for the capabilities.
    pub(crate) async fn capability(&mut self) -> SyncResult<Capabilities> {
        let (untagged, done) = self.command("CAPABILITY").await?;
        if done.status != Status::Ok {
            return Err(self.error(done.text));
        }
        untagged
            .iter()
            .find_map(|line| {
                let (name, atoms) = line.split_once(' ')?;
                name.eq_ignore_ascii_case("CAPABILITY")
                    .then(|| Capabilities::parse(atoms))
            })
            .ok_or_else(|| self.error("the server did not list its capabilities"))
    }

    /// Asks to start TLS. The stream, ready for the handshake.
    pub(crate) async fn starttls(mut self) -> SyncResult<S> {
        let (_, done) = self.command("STARTTLS").await?;
        if done.status != Status::Ok {
            return Err(SyncError::Insecure {
                server: self.server.clone(),
            });
        }
        self.into_inner()
    }

    /// The stream, once nothing more is waiting to be read: anything the
    /// server sent unasked at this point is refused rather than passed on.
    pub(crate) fn into_inner(self) -> SyncResult<S> {
        if !self.reader.buffer().is_empty() {
            return Err(self.error("the server sent more than it was asked for"));
        }
        Ok(self.reader.into_inner())
    }
}

/// The capabilities in a `[CAPABILITY ...]` response code at the start of
/// `text`.
fn capability_code(text: &str) -> Option<Capabilities> {
    let inner = text.strip_prefix('[')?;
    let (code, _) = inner.split_once(']')?;
    let (name, atoms) = code.split_once(' ')?;
    name.eq_ignore_ascii_case("CAPABILITY")
        .then(|| Capabilities::parse(atoms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_in_a_greeting() {
        let caps = capability_code("[CAPABILITY IMAP4rev1 STARTTLS] ready").unwrap();
        assert!(caps.starttls());
        assert!(capability_code("ready").is_none());
        assert!(capability_code("[ALERT] hi").is_none());
    }

    #[tokio::test]
    async fn greeting_command_and_leftovers() {
        let (client, mut server) = tokio::io::duplex(4096);
        let mut pre = PreAuth::new(client, "imap.example.org");
        server
            .write_all(b"* OK [CAPABILITY IMAP4rev1 STARTTLS] hi\r\n")
            .await
            .unwrap();
        assert!(pre.greeting().await.unwrap().unwrap().starttls());

        // The answer to STARTTLS with a command smuggled in after it.
        server
            .write_all(b"p1 OK go ahead\r\n* OK injected\r\n")
            .await
            .unwrap();
        let err = pre.starttls().await.err().unwrap();
        assert!(matches!(err, SyncError::Server { .. }), "{err:?}");

        let mut sent = vec![0; 64];
        let n = server.read(&mut sent).await.unwrap();
        assert_eq!(&sent[..n], b"p1 STARTTLS\r\n");
    }

    async fn greeted_with(greeting: &[u8]) -> SyncError {
        let (client, mut server) = tokio::io::duplex(4096);
        let mut pre = PreAuth::new(client, "imap.example.org");
        server.write_all(greeting).await.unwrap();
        pre.greeting().await.err().unwrap()
    }

    #[tokio::test]
    async fn bye_and_preauth_greetings_are_refused() {
        for (greeting, expected) in [
            (&b"* BYE Shutting down\r\n"[..], "Shutting down"),
            (
                b"* BYE [SERVERBUG] Too many simultaneous connections\r\n",
                "Too many",
            ),
            (b"* PREAUTH welcome\r\n", "signed in on its own"),
        ] {
            let err = greeted_with(greeting).await;
            assert!(matches!(err, SyncError::Server { .. }), "{err:?}");
            assert!(err.to_string().contains(expected), "{err}");
        }
    }

    #[tokio::test]
    async fn a_bye_greeting_at_a_limit_is_busy() {
        for (greeting, expected) in [
            (&b"* BYE [UNAVAILABLE] Too busy\r\n"[..], "Too busy"),
            (
                b"* BYE Maximum number of connections from user+IP exceeded (mail_max_userip_connections=10)\r\n",
                "Maximum number of connections from user+IP exceeded (mail_max_userip_connections=10)",
            ),
        ] {
            match greeted_with(greeting).await {
                SyncError::Busy { message, .. } => assert_eq!(message, expected),
                other => panic!("expected busy, got {other:?}"),
            }
        }
    }
}
