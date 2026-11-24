//! Probing a guessed server before it is offered: a trait, so tests can
//! answer, and the real network prober.
//!
//! A TLS server (993, 465) must complete a TLS handshake that the
//! platform's verifier accepts, then greet as its protocol does. A STARTTLS
//! server (143, 587) must greet, offer STARTTLS, accept it, and then
//! complete the same verified handshake. No one signs in.
//!
//! The server's name is resolved and its addresses tried as RFC 8305 says
//! ([`connect`]), so one unreachable address does not use up the probe's
//! time.

mod connect;
mod dialogue;

use std::sync::Arc;

use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use rustls_platform_verifier::BuilderVerifierExt;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::BoxFuture;
use crate::types::Security;

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
    /// The server offered STARTTLS, then refused it.
    #[error("the server refused STARTTLS: {0:?}")]
    StartTlsRefused(String),
    /// An IMAP server on a STARTTLS port greeted with `* PREAUTH`: it signs
    /// the connection in before TLS can start, so TLS never can.
    #[error("the server signs in before TLS (PREAUTH)")]
    PreAuth,
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
        Ok(Self::with_tls_config(Arc::new(config)))
    }

    /// A prober verifying certificates as `config` says instead: tests use
    /// this to trust a root of their own.
    pub fn with_tls_config(config: Arc<ClientConfig>) -> Self {
        NetProbe {
            tls: TlsConnector::from(config),
        }
    }

    /// The TLS handshake, the certificate verified for `host`.
    async fn handshake<S>(&self, host: &str, stream: S) -> Result<TlsStream<S>, ProbeError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let name = ServerName::try_from(host.to_owned())
            .map_err(|e| ProbeError::Connect(e.to_string()))?;
        self.tls
            .connect(name, stream)
            .await
            .map_err(|e| ProbeError::Connect(e.to_string()))
    }

    async fn run(
        &self,
        protocol: Protocol,
        host: &str,
        port: u16,
        security: Security,
    ) -> Result<(), ProbeError> {
        let tcp = connect::connect(host, port)
            .await
            .map_err(|e| ProbeError::Connect(e.to_string()))?;
        match security {
            Security::Tls => {
                let mut stream = BufReader::new(self.handshake(host, tcp).await?);
                match protocol {
                    Protocol::Imap => dialogue::imap_greeting(&mut stream).await.map(drop),
                    Protocol::Smtp => dialogue::smtp_reply(&mut stream, 220).await.map(drop),
                }
            }
            Security::StartTls => {
                let local = tcp.local_addr()?.ip();
                let mut plain = BufReader::new(tcp);
                match protocol {
                    Protocol::Imap => dialogue::imap_starttls(&mut plain).await?,
                    Protocol::Smtp => dialogue::smtp_starttls(&mut plain, local).await?,
                }
                // Bytes sent after the go-ahead but before the handshake
                // could be taken for the server's answers over TLS.
                if !plain.buffer().is_empty() {
                    return Err(ProbeError::Io(
                        "the server sent data before TLS began".to_owned(),
                    ));
                }
                let mut tls = self.handshake(host, plain.into_inner()).await?;
                // Courtesy only: the probe's answer is already known.
                let goodbye: &[u8] = match protocol {
                    Protocol::Imap => b"P3 LOGOUT\r\n",
                    Protocol::Smtp => b"QUIT\r\n",
                };
                if tls.write_all(goodbye).await.is_ok() {
                    let _ = tls.flush().await;
                }
                Ok(())
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
