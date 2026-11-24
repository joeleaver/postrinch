//! A test certificate authority, and scripted servers on loopback that
//! speak a protocol's plain-text part, then STARTTLS with a certificate the
//! authority signed.

use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// A root certificate of the test's own, and what it signs.
pub struct Authority {
    root: CertificateDer<'static>,
    issuer: Issuer<'static, KeyPair>,
}

impl Authority {
    pub fn new() -> Self {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(DnType::CommonName, "Postrinch test root");
        let root = params.self_signed(&key).unwrap().der().clone();
        Authority {
            root,
            issuer: Issuer::new(params, key),
        }
    }

    /// A client configuration trusting this root only.
    pub fn client(&self) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots.add(self.root.clone()).unwrap();
        let config = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    }

    /// A server configuration with a certificate for `names` (host names
    /// or IP addresses), signed by this root.
    pub fn server(&self, names: &[&str]) -> Arc<ServerConfig> {
        let key = KeyPair::generate().unwrap();
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        let cert = CertificateParams::new(names)
            .unwrap()
            .signed_by(&key, &self.issuer)
            .unwrap();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], key)
            .unwrap();
        Arc::new(config)
    }
}

/// How a STARTTLS server's session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// The client never asked for STARTTLS.
    NoStartTls,
    /// The handshake failed.
    HandshakeFailed,
    /// TLS began; the first line the client sent over it (empty when it
    /// sent none).
    OverTls(String),
}

/// A server on loopback that sends `greeting`, answers each line with the
/// first reply whose prefix it starts with, and when a line starts with
/// `starttls`, sends `go_ahead` and starts TLS with `tls`. Returns its port
/// and how the session ended.
pub async fn starttls_server(
    greeting: &'static str,
    replies: &'static [(&'static str, &'static str)],
    starttls: &'static str,
    go_ahead: &'static str,
    tls: Arc<ServerConfig>,
) -> (u16, JoinHandle<Ended>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let session = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(socket);
        stream
            .get_mut()
            .write_all(greeting.as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                return Ended::NoStartTls;
            }
            let upper = line.to_ascii_uppercase();
            if upper.starts_with(starttls) {
                stream
                    .get_mut()
                    .write_all(go_ahead.as_bytes())
                    .await
                    .unwrap();
                break;
            }
            match replies.iter().find(|(prefix, _)| upper.starts_with(prefix)) {
                Some((_, reply)) => stream.get_mut().write_all(reply.as_bytes()).await.unwrap(),
                None => return Ended::NoStartTls,
            }
        }
        let Ok(secure) = TlsAcceptor::from(tls).accept(stream.into_inner()).await else {
            return Ended::HandshakeFailed;
        };
        let mut first = String::new();
        let _ = BufReader::new(secure).read_line(&mut first).await;
        Ended::OverTls(first.trim_end().to_owned())
    });
    (port, session)
}
