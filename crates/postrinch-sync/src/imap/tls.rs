//! TLS for IMAP and SMTP: rustls with aws-lc-rs, certificates checked by
//! the platform's verifier, as reqwest and discovery check them.
//!
//! The roots trusted beyond the platform's are the one thing a caller can
//! change ([`TlsConfig::with_extra_roots`]): tests trust their fake
//! server's certificate that way. There is no switch to accept a
//! certificate that does not verify.

use std::sync::Arc;
use std::time::Duration;

use lettre::transport::smtp::client::{Certificate, TlsParameters};
use rustls::ClientConfig;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls_platform_verifier::Verifier;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::{SyncError, SyncResult};

/// How long to wait for a mail server. Generous enough for a slow server
/// on a slow link; short enough that a dead one is reported while the
/// person still waits on the add-account dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// For the TCP connection and the TLS handshake.
    pub connect: Duration,
    /// For each exchange with the server after that: the greeting, a
    /// command and its answer, a sign-in.
    pub command: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(20),
            command: Duration::from_secs(60),
        }
    }
}

/// The TLS settings for mail servers.
#[derive(Clone)]
pub struct TlsConfig {
    extra_roots: Vec<CertificateDer<'static>>,
    connector: TlsConnector,
}

impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConfig")
            .field("extra_roots", &self.extra_roots.len())
            .finish_non_exhaustive()
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn tls_error(detail: impl std::fmt::Display) -> SyncError {
    SyncError::Tls {
        server: "mail servers".into(),
        detail: detail.to_string(),
    }
}

impl TlsConfig {
    /// Trusts what the platform trusts.
    pub fn platform() -> SyncResult<Self> {
        Self::with_extra_roots(Vec::new())
    }

    /// Trusts what the platform trusts and these roots too.
    pub fn with_extra_roots(extra_roots: Vec<CertificateDer<'static>>) -> SyncResult<Self> {
        let provider = provider();
        let verifier = if extra_roots.is_empty() {
            Verifier::new(provider.clone())
        } else {
            Verifier::new_with_extra_roots(extra_roots.clone(), provider.clone())
        }
        .map_err(tls_error)?;
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(tls_error)?
            // "dangerous" is rustls's name for any verifier of one's own;
            // this one is the platform's, which checks everything.
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        Ok(Self {
            extra_roots,
            connector: TlsConnector::from(Arc::new(config)),
        })
    }

    /// The TLS handshake over `tcp`, the certificate verified for `host`.
    pub(crate) async fn handshake(
        &self,
        host: &str,
        tcp: TcpStream,
    ) -> SyncResult<TlsStream<TcpStream>> {
        let name = ServerName::try_from(host.to_owned()).map_err(|e| SyncError::Tls {
            server: host.into(),
            detail: e.to_string(),
        })?;
        self.connector
            .connect(name, tcp)
            .await
            .map_err(|e| SyncError::Tls {
                server: host.into(),
                detail: e.to_string(),
            })
    }

    /// The same trust for lettre's SMTP connections to `host`.
    pub(crate) fn smtp_parameters(&self, host: &str) -> SyncResult<TlsParameters> {
        let mut builder = TlsParameters::builder(host.to_owned());
        for root in &self.extra_roots {
            let certificate = Certificate::from_der(root.to_vec()).map_err(tls_error)?;
            builder = builder.add_root_certificate(certificate);
        }
        builder.build_rustls().map_err(|e| SyncError::Tls {
            server: host.into(),
            detail: e.to_string(),
        })
    }
}
