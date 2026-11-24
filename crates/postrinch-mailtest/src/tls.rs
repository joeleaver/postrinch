//! The fakes' TLS: a self-signed certificate made at startup, and the
//! rustls configurations that serve it and trust it.
//!
//! The workspace links both of rustls's crypto providers (ring through
//! some dependencies, aws-lc-rs through others), so rustls cannot pick a
//! process default on its own. Every configuration here names aws-lc-rs
//! explicitly, as Postrinch itself does, and tests never depend on a
//! process-wide default being installed.

use std::sync::Arc;

use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::TlsAcceptor;

use crate::Error;

/// The host name the test certificate is issued for. Clients connect to
/// `127.0.0.1` and verify the certificate against this name (the
/// certificate also names the address itself).
pub const HOST: &str = "localhost";

/// The crypto provider every configuration in this crate uses.
pub fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// A self-signed certificate for `localhost` and `127.0.0.1`, made fresh
/// for each server, with its private key.
pub struct TestCertificate {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
}

impl Clone for TestCertificate {
    fn clone(&self) -> Self {
        Self {
            cert: self.cert.clone(),
            key: self.key.clone_key(),
        }
    }
}

impl std::fmt::Debug for TestCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestCertificate")
            .field("cert_len", &self.cert.len())
            .finish_non_exhaustive()
    }
}

impl TestCertificate {
    /// Makes a new key pair and a certificate signed by it.
    pub fn generate() -> Result<Self, Error> {
        let names = vec![HOST.to_string(), "127.0.0.1".to_string()];
        let certified = rcgen::generate_simple_self_signed(names)
            .map_err(|e| Error::Tls(format!("making the test certificate: {e}")))?;
        Ok(Self {
            cert: certified.cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()),
        })
    }

    /// The certificate, DER-encoded: what a client adds to its trusted roots.
    pub fn der(&self) -> &CertificateDer<'static> {
        &self.cert
    }

    /// A root store trusting only this certificate.
    pub fn root_store(&self) -> RootCertStore {
        let mut roots = RootCertStore::empty();
        roots
            .add(self.cert.clone())
            .expect("a certificate rcgen just made is valid DER");
        roots
    }

    /// A client configuration (aws-lc-rs, TLS 1.2 and 1.3) trusting only
    /// this certificate, ready for `tokio_rustls::TlsConnector`.
    pub fn client_config(&self) -> Result<Arc<ClientConfig>, Error> {
        let config = ClientConfig::builder_with_provider(crypto_provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Tls(e.to_string()))?
            .with_root_certificates(self.root_store())
            .with_no_client_auth();
        Ok(Arc::new(config))
    }

    /// The name clients should verify the certificate against.
    pub fn server_name() -> ServerName<'static> {
        ServerName::try_from(HOST).expect("localhost is a valid DNS name")
    }

    /// The server side: an acceptor presenting this certificate.
    pub(crate) fn acceptor(&self) -> Result<TlsAcceptor, Error> {
        let key = PrivateKeyDer::Pkcs8(self.key.clone_key());
        let config = ServerConfig::builder_with_provider(crypto_provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Tls(e.to_string()))?
            .with_no_client_auth()
            .with_single_cert(vec![self.cert.clone()], key)
            .map_err(|e| Error::Tls(e.to_string()))?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}
