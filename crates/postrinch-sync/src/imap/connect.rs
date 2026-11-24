//! Reaching an IMAP server: TCP, then TLS from the first byte (993) or
//! after STARTTLS (143), then the greeting and the capabilities.
//!
//! Plain text is never spoken past STARTTLS: a server on a STARTTLS port
//! that does not offer it, or refuses it, gets no credentials
//! ([`SyncError::Insecure`]).

use std::future::Future;

use postrinch_model::{Security, Server};
use tokio::net::TcpStream;

use super::capabilities::Capabilities;
use super::preauth::PreAuth;
use super::reply::{ImapStream, Recorder};
use super::tls::{Timeouts, TlsConfig};
use crate::{SyncError, SyncResult};

/// An IMAP client that has not signed in yet.
pub(crate) type Client = async_imap::Client<ImapStream>;

/// Connects to mail servers: the TLS trust and the timeouts to use.
#[derive(Debug, Clone)]
pub struct Connector {
    tls: TlsConfig,
    timeouts: Timeouts,
}

/// A connection over TLS whose greeting has been read.
pub(crate) struct Greeted {
    pub(crate) client: Client,
    /// What the server offers before sign-in.
    pub(crate) capabilities: Capabilities,
    pub(crate) recorder: Recorder,
}

/// "host:port", as errors name a server.
pub(crate) fn describe(server: &Server) -> String {
    format!("{}:{}", server.host, server.port)
}

impl Connector {
    pub fn new(tls: TlsConfig) -> Self {
        Self {
            tls,
            timeouts: Timeouts::default(),
        }
    }

    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    pub fn tls(&self) -> &TlsConfig {
        &self.tls
    }

    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// Runs one exchange with a server, within the command timeout.
    pub(crate) async fn exchange<T>(
        &self,
        server: &Server,
        what: &str,
        future: impl Future<Output = SyncResult<T>>,
    ) -> SyncResult<T> {
        within(self.timeouts.command, server, what, future).await
    }

    /// Connects to an IMAP server and reads its greeting.
    pub(crate) async fn imap(&self, server: &Server) -> SyncResult<Greeted> {
        let name = describe(server);
        let connect = async {
            let tcp = TcpStream::connect((server.host.as_str(), server.port))
                .await
                .map_err(|e| SyncError::Unreachable {
                    server: name.clone(),
                    detail: e.to_string(),
                })?;
            // Small commands answered at once: no waiting to fill packets.
            tcp.set_nodelay(true)?;
            match server.security {
                Security::Tls => self.tls.handshake(&server.host, tcp).await,
                Security::StartTls => self.starttls(server, tcp).await,
            }
        };
        let tls = within(self.timeouts.connect, server, "connecting", connect).await?;
        self.exchange(server, "reading the greeting", async {
            let mut pre = PreAuth::new(tls, &name);
            // After STARTTLS there is no second greeting (RFC 9051 6.2.1).
            let greeting = match server.security {
                Security::Tls => pre.greeting().await?,
                Security::StartTls => None,
            };
            let capabilities = match greeting {
                Some(capabilities) => capabilities,
                None => pre.capability().await?,
            };
            let (stream, recorder) = ImapStream::new(pre.into_inner()?);
            Ok(Greeted {
                client: async_imap::Client::new(stream),
                capabilities,
                recorder,
            })
        })
        .await
    }

    /// The plain-text start of a STARTTLS connection, up to the handshake.
    async fn starttls(
        &self,
        server: &Server,
        tcp: TcpStream,
    ) -> SyncResult<tokio_rustls::client::TlsStream<TcpStream>> {
        let mut pre = PreAuth::new(tcp, &describe(server));
        let greeting = pre.greeting().await?;
        let capabilities = match greeting {
            Some(capabilities) => capabilities,
            None => pre.capability().await?,
        };
        if !capabilities.starttls() {
            return Err(SyncError::Insecure {
                server: describe(server),
            });
        }
        let tcp = pre.starttls().await?;
        self.tls.handshake(&server.host, tcp).await
    }
}

/// Runs `future` within `limit`, naming the server when it takes too long.
pub(crate) async fn within<T>(
    limit: std::time::Duration,
    server: &Server,
    what: &str,
    future: impl Future<Output = SyncResult<T>>,
) -> SyncResult<T> {
    tokio::time::timeout(limit, future)
        .await
        .unwrap_or_else(|_| {
            Err(SyncError::Unreachable {
                server: describe(server),
                detail: format!("no answer while {what} in {} seconds", limit.as_secs()),
            })
        })
}
