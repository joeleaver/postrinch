//! What the transport trusts: the platform's roots, plus any extra ones.
//!
//! HTTPS goes through reqwest's rustls with the platform verifier; the
//! WebSocket opens its own TLS stream with the same crypto provider and
//! verifier, so both trust exactly the same certificates.

use std::sync::Arc;

use rustls::ClientConfig;
use rustls::pki_types::CertificateDer;
use rustls_platform_verifier::Verifier;

/// The trust settings for every connection to providers.
#[derive(Clone)]
pub struct Tls {
    extra_roots: Vec<CertificateDer<'static>>,
    /// For the WebSocket's TLS stream (HTTP/1.1 only, as the upgrade needs).
    ws: Arc<ClientConfig>,
}

/// The TLS settings could not be made.
#[derive(Debug, thiserror::Error)]
#[error("cannot set up TLS: {0}")]
pub struct TlsError(String);

impl TlsError {
    pub(crate) fn new(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tls")
            .field("extra_roots", &self.extra_roots.len())
            .finish_non_exhaustive()
    }
}

impl Tls {
    /// Trust what the platform trusts.
    pub fn platform() -> Result<Self, TlsError> {
        Self::with_extra_roots(Vec::new())
    }

    /// Trust what the platform trusts and these roots too, each DER-encoded.
    /// Tests use this for their fake provider's root.
    pub fn with_extra_roots(extra_roots: Vec<Vec<u8>>) -> Result<Self, TlsError> {
        let extra_roots: Vec<CertificateDer<'static>> =
            extra_roots.into_iter().map(CertificateDer::from).collect();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = if extra_roots.is_empty() {
            Verifier::new(provider.clone())
        } else {
            Verifier::new_with_extra_roots(extra_roots.clone(), provider.clone())
        }
        .map_err(TlsError::new)?;
        let mut ws = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(TlsError::new)?
            // "dangerous" is rustls's name for any verifier of one's own;
            // this one is the platform's, which checks everything.
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        ws.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            extra_roots,
            ws: Arc::new(ws),
        })
    }

    /// The extra roots, for reqwest.
    pub(crate) fn extra_roots(&self) -> &[CertificateDer<'static>] {
        &self.extra_roots
    }

    /// The configuration of the WebSocket's TLS stream.
    pub(crate) fn ws_config(&self) -> Arc<ClientConfig> {
        self.ws.clone()
    }
}
