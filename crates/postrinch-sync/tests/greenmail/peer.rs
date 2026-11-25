//! Another mail client on the same GreenMail, independent of Postrinch's
//! IMAP code: async-imap over tokio-rustls, trusting the run's CA. It puts
//! mail and folders on the server as a person's other client would, and
//! reads back what the server holds, so the smoke test checks the server,
//! not Postrinch's idea of it.

use std::net::SocketAddr;
use std::sync::Arc;

use async_imap::Session;
use async_imap::types::Flag;
use futures_util::TryStreamExt;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

type Imap = Session<TlsStream<TcpStream>>;

/// A message on the server, as another client sees it.
#[derive(Debug)]
pub struct Seen {
    pub message_id: String,
    pub subject: String,
    pub flags: Vec<String>,
}

impl Seen {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

/// A signed-in IMAP session of the other client.
pub struct Peer {
    session: Imap,
}

impl Peer {
    /// Signs in to the IMAPS server at `addr` as `user`.
    pub async fn sign_in(
        addr: SocketAddr,
        ca: &CertificateDer<'static>,
        user: &str,
        password: &str,
    ) -> Self {
        let mut roots = RootCertStore::empty();
        roots.add(ca.clone()).expect("the CA's certificate");
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = TcpStream::connect(addr)
            .await
            .expect("GreenMail's IMAPS port");
        let name = ServerName::IpAddress(addr.ip().into());
        let tls = TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .expect("a TLS connection to GreenMail");
        let mut client = async_imap::Client::new(tls);
        client
            .read_response()
            .await
            .expect("the greeting")
            .expect("a greeting");
        let session = client
            .login(user, password)
            .await
            .map_err(|(e, _)| e)
            .expect("the other client signs in");
        Self { session }
    }

    pub async fn create(&mut self, folder: &str) {
        self.session.create(folder).await.expect("CREATE");
    }

    /// Appends a plain message with this `Message-ID` and subject, from
    /// Ann, its body `body`.
    pub async fn append(&mut self, folder: &str, message_id: &str, subject: &str, body: &str) {
        let raw = format!(
            "From: Ann <ann@example.com>\r\nTo: ada@example.org\r\nSubject: {subject}\r\n\
             Date: Tue, 24 Nov 2026 09:00:00 +0000\r\nMessage-ID: <{message_id}>\r\n\
             MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        );
        self.session
            .append(folder, None, None, raw.as_bytes())
            .await
            .expect("APPEND");
    }

    /// Every message in `folder`: its `Message-ID`, subject and flags.
    pub async fn messages(&mut self, folder: &str) -> Vec<Seen> {
        let mailbox = self.session.select(folder).await.expect("SELECT");
        if mailbox.exists == 0 {
            return Vec::new();
        }
        let fetches: Vec<_> = self
            .session
            .fetch("1:*", "(FLAGS BODY.PEEK[HEADER])")
            .await
            .expect("FETCH")
            .try_collect()
            .await
            .expect("FETCH's answers");
        fetches
            .iter()
            .map(|fetch| Seen {
                message_id: header(fetch.header().unwrap_or_default(), "message-id"),
                subject: header(fetch.header().unwrap_or_default(), "subject"),
                flags: fetch.flags().map(flag_name).collect(),
            })
            .collect()
    }

    /// The messages in `folder` with these `Message-ID`s (without angle
    /// brackets), in the order asked; panics naming what is missing.
    pub async fn find(&mut self, folder: &str, ids: &[&str]) -> Vec<Seen> {
        let mut all = self.messages(folder).await;
        ids.iter()
            .map(|id| {
                let at = all
                    .iter()
                    .position(|m| m.message_id == format!("<{id}>"))
                    .unwrap_or_else(|| panic!("{id} is not in {folder}: {all:?}"));
                all.remove(at)
            })
            .collect()
    }

    /// The `Message-ID`s (without angle brackets) of the messages in
    /// `folder`, sorted.
    pub async fn ids(&mut self, folder: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .messages(folder)
            .await
            .into_iter()
            .map(|m| m.message_id.trim_matches(['<', '>']).to_owned())
            .collect();
        ids.sort();
        ids
    }

    pub async fn sign_out(mut self) {
        self.session.logout().await.expect("LOGOUT");
    }
}

fn flag_name(flag: Flag<'_>) -> String {
    match flag {
        Flag::Seen => "\\Seen".into(),
        Flag::Answered => "\\Answered".into(),
        Flag::Flagged => "\\Flagged".into(),
        Flag::Deleted => "\\Deleted".into(),
        Flag::Draft => "\\Draft".into(),
        Flag::Recent => "\\Recent".into(),
        Flag::MayCreate => "\\*".into(),
        Flag::Custom(name) => name.into_owned(),
    }
}

/// The value of the header `name` (lower case) in `raw`, unfolded, or
/// empty.
fn header(raw: &[u8], name: &str) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut value: Option<String> = None;
    for line in text.split("\r\n") {
        match value.as_mut() {
            Some(v) if line.starts_with([' ', '\t']) => v.push_str(line.trim_start()),
            Some(_) => break,
            None => {
                if let Some((field, rest)) = line.split_once(':')
                    && field.eq_ignore_ascii_case(name)
                {
                    value = Some(rest.trim().to_owned());
                }
            }
        }
    }
    value.unwrap_or_default()
}
