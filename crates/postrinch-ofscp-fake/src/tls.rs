//! The fakes' TLS: one self-signed certificate per test process, for
//! `127.0.0.1` and `localhost`, served by every fake provider.
//!
//! OFSCP is https-only (§3.2, §5.8 attachment URLs, §4.6 key fetches), so
//! the fakes are too: a client under test builds `https://{authority}`
//! exactly as it does in production and only adds
//! [`root_certificate_der`] to its trusted roots. Since every provider in
//! the process serves the same certificate, one root trusts them all.
//!
//! The workspace links both of rustls's crypto providers, so rustls cannot
//! pick a process default; every configuration here names aws-lc-rs, as
//! Postrinch itself does (see `postrinch-mailtest`'s `tls.rs`).

use std::sync::{Arc, OnceLock};

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

struct TestCertificate {
    der: CertificateDer<'static>,
    pem: String,
    key: PrivatePkcs8KeyDer<'static>,
}

fn certificate() -> &'static TestCertificate {
    static CERT: OnceLock<TestCertificate> = OnceLock::new();
    CERT.get_or_init(|| {
        let names = vec!["127.0.0.1".to_string(), "localhost".to_string()];
        let certified = rcgen::generate_simple_self_signed(names)
            .expect("rcgen makes a self-signed certificate for fixed names");
        TestCertificate {
            der: certified.cert.der().clone(),
            pem: certified.cert.pem(),
            key: PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()),
        }
    })
}

/// The certificate every fake provider serves, DER-encoded: add it to a
/// client's trusted roots (reqwest's `Certificate::from_der`, or a rustls
/// `RootCertStore`).
pub fn root_certificate_der() -> &'static [u8] {
    certificate().der.as_ref()
}

/// The same certificate, PEM-encoded.
pub fn root_certificate_pem() -> &'static str {
    &certificate().pem
}

fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// The rustls server configuration every fake serves. HTTP/1.1 only: the
/// WebSocket upgrade needs it, and nothing in OFSCP needs HTTP/2.
pub(crate) fn server_config() -> Arc<ServerConfig> {
    let cert = certificate();
    let mut config = ServerConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .expect("aws-lc-rs supports the default TLS versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der.clone()],
            PrivateKeyDer::Pkcs8(cert.key.clone_key()),
        )
        .expect("the test certificate matches its key");
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// The HTTP client a fake provider makes its own outbound requests with
/// (§4.6 key fetches from other fakes). It trusts only the test
/// certificate, uses no proxy, and follows no redirects (§8 *Outbound
/// requests* treats a `3xx` as a failed request).
pub(crate) fn outbound_client() -> reqwest::Client {
    let cert = reqwest::Certificate::from_der(root_certificate_der())
        .expect("the test certificate is valid DER");
    reqwest::Client::builder()
        .tls_certs_only([cert])
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("a reqwest client with one trusted root builds")
}
