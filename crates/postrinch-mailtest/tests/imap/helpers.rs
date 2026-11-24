//! Connecting test clients to the fake IMAP server.

use std::net::SocketAddr;

use postrinch_mailtest::{ImapServer, TestCertificate};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

pub type Tls = TlsStream<TcpStream>;

pub const MAIL_1: &[u8] = b"From: Ann <ann@example.com>\r\nTo: bob@example.com\r\nSubject: Lunch plans\r\nDate: Tue, 24 Nov 2026 09:00:00 +0000\r\nMessage-ID: <one@example.com>\r\n\r\nShall we meet at noon?\r\n";
pub const MAIL_2: &[u8] = b"From: Cy <cy@example.com>\r\nTo: bob@example.com\r\nSubject: Invoice\r\nDate: Wed, 25 Nov 2026 10:00:00 +0000\r\nMessage-ID: <two@example.com>\r\n\r\nPlease find the invoice attached.\r\n";

/// Opens TLS to `addr`, trusting only `cert`.
pub async fn tls_to(addr: SocketAddr, cert: &TestCertificate) -> Tls {
    let tcp = TcpStream::connect(addr).await.expect("connect");
    tls_over(tcp, cert).await
}

/// Starts TLS on an open connection, trusting only `cert`.
pub async fn tls_over(tcp: TcpStream, cert: &TestCertificate) -> Tls {
    let connector = TlsConnector::from(cert.client_config().expect("client config"));
    connector
        .connect(TestCertificate::server_name(), tcp)
        .await
        .expect("TLS handshake")
}

/// An async-imap client, greeted, not yet signed in.
pub async fn imap_client(server: &ImapServer) -> async_imap::Client<Tls> {
    let stream = tls_to(server.tls_addr().expect("TLS port"), server.certificate()).await;
    let mut client = async_imap::Client::new(stream);
    client
        .read_response()
        .await
        .expect("greeting")
        .expect("greeting");
    client
}

/// An async-imap session signed in with LOGIN.
pub async fn imap_login(
    server: &ImapServer,
    user: &str,
    password: &str,
) -> async_imap::Session<Tls> {
    imap_client(server)
        .await
        .login(user, password)
        .await
        .map_err(|(e, _)| e)
        .expect("login")
}

/// A line-level IMAP client for checking exact responses.
pub struct Raw {
    stream: BufReader<Tls>,
    next_tag: u32,
}

impl Raw {
    /// Connects over TLS and reads the greeting.
    pub async fn connect(server: &ImapServer) -> Raw {
        let stream = tls_to(server.tls_addr().expect("TLS port"), server.certificate()).await;
        let mut raw = Raw {
            stream: BufReader::new(stream),
            next_tag: 1,
        };
        let greeting = raw.line().await;
        assert!(greeting.starts_with("* OK"), "{greeting}");
        raw
    }

    /// Connects and logs in.
    pub async fn login(server: &ImapServer, user: &str, password: &str) -> Raw {
        let mut raw = Raw::connect(server).await;
        let lines = raw.command(&format!("LOGIN {user} {password}")).await;
        assert!(lines.last().unwrap().contains("OK"), "{lines:?}");
        raw
    }

    /// Reads one line, without its CRLF. Literals in it are read whole and
    /// shown inline after their `{n}`.
    pub async fn line(&mut self) -> String {
        let mut line = String::new();
        loop {
            let mut part = String::new();
            let n = self.stream.read_line(&mut part).await.expect("read");
            assert!(n > 0, "server closed the connection; so far: {line:?}");
            let part = part.trim_end_matches(['\r', '\n']).to_string();
            line.push_str(&part);
            let literal = part
                .strip_suffix('}')
                .and_then(|p| p.rsplit_once('{'))
                .and_then(|(_, n)| n.parse::<usize>().ok());
            match literal {
                Some(size) => {
                    let mut bytes = vec![0; size];
                    self.stream.read_exact(&mut bytes).await.expect("literal");
                    line.push_str(&String::from_utf8_lossy(&bytes));
                }
                None => return line,
            }
        }
    }

    /// Whether the server has closed the connection (reading gives EOF).
    pub async fn closed(&mut self) -> bool {
        let mut rest = String::new();
        matches!(self.stream.read_line(&mut rest).await, Ok(0) | Err(_))
    }

    pub async fn send(&mut self, bytes: &[u8]) {
        self.stream.get_mut().write_all(bytes).await.expect("write");
        self.stream.get_mut().flush().await.expect("flush");
    }

    /// Sends a tagged command and returns every line up to and including
    /// the tagged reply.
    pub async fn command(&mut self, command: &str) -> Vec<String> {
        let tag = format!("t{}", self.next_tag);
        self.next_tag += 1;
        self.send(format!("{tag} {command}\r\n").as_bytes()).await;
        self.until(&tag).await
    }

    /// Reads lines up to and including the one tagged `tag`.
    pub async fn until(&mut self, tag: &str) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            let line = self.line().await;
            let done = line.starts_with(&format!("{tag} "));
            lines.push(line);
            if done {
                return lines;
            }
        }
    }
}
