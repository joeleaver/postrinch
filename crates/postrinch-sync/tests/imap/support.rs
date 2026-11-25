//! What the IMAP connection tests share: an account, settings pointing at
//! the fake servers, a connector trusting their certificate, and access
//! tokens a test controls.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use postrinch_mailtest::TestCertificate;
use postrinch_model::{
    EmailAddress, FolderRoles, ImapSettings, Security, Server, SignInMethod, SignInReason,
};
use postrinch_sync::imap::{AccessTokens, Connector, ImapSecret, Timeouts, TlsConfig, TokenFuture};
use postrinch_sync::{SyncError, SyncResult};

/// The account's address, and its user name on both fakes.
pub const USER: &str = "ada@example.org";
pub const PASSWORD: &str = "correct horse";

pub fn account() -> EmailAddress {
    EmailAddress::parse(USER).unwrap()
}

/// A connector trusting the fake's certificate, with short timeouts.
pub fn connector(certificate: &TestCertificate) -> Connector {
    let tls = TlsConfig::with_extra_roots(vec![certificate.der().clone()]).unwrap();
    Connector::new(tls).with_timeouts(Timeouts {
        connect: Duration::from_secs(5),
        command: Duration::from_secs(5),
    })
}

/// A server at a fake's loopback address.
pub fn server(addr: SocketAddr, security: Security) -> Server {
    Server {
        host: addr.ip().to_string(),
        port: addr.port(),
        security,
        username: USER.into(),
    }
}

pub fn settings(imap: Server, smtp: Server, sign_in: SignInMethod) -> ImapSettings {
    ImapSettings {
        imap,
        smtp,
        sign_in,
        roles: FolderRoles::default(),
        server_files_sent: true,
    }
}

pub fn microsoft() -> SignInMethod {
    SignInMethod::OAuth2 {
        provider: "microsoft".into(),
    }
}

pub fn oauth_secret() -> ImapSecret {
    ImapSecret::OAuth2 {
        provider: "microsoft".into(),
        refresh_token: "refresh".into(),
    }
}

/// An address nothing listens on.
pub async fn closed_port() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    listener.local_addr().unwrap()
}

/// Access tokens in order: the first until it is invalidated, then the
/// next, and so on. With none left, the provider refuses the grant.
pub struct Tokens {
    tokens: Vec<String>,
    current: Mutex<usize>,
    pub fetched: AtomicUsize,
    pub invalidated: AtomicUsize,
}

impl Tokens {
    pub fn new(tokens: &[&str]) -> Self {
        Self {
            tokens: tokens.iter().map(|t| t.to_string()).collect(),
            current: Mutex::new(0),
            fetched: AtomicUsize::new(0),
            invalidated: AtomicUsize::new(0),
        }
    }

    /// No OAuth at all: for password accounts.
    pub fn none() -> Self {
        Self::new(&[])
    }

    pub fn fetched(&self) -> usize {
        self.fetched.load(Ordering::SeqCst)
    }

    pub fn invalidated(&self) -> usize {
        self.invalidated.load(Ordering::SeqCst)
    }
}

impl AccessTokens for Tokens {
    fn access_token<'a>(&'a self, _: &'a EmailAddress) -> TokenFuture<'a, SyncResult<String>> {
        Box::pin(async move {
            self.fetched.fetch_add(1, Ordering::SeqCst);
            let current = *self.current.lock().unwrap();
            self.tokens.get(current).cloned().ok_or_else(|| {
                SyncError::NeedsSignIn(SignInReason::CredentialRejected {
                    detail: "invalid_grant".into(),
                })
            })
        })
    }

    fn invalidate<'a>(&'a self, _: &'a EmailAddress) -> TokenFuture<'a, ()> {
        Box::pin(async move {
            self.invalidated.fetch_add(1, Ordering::SeqCst);
            *self.current.lock().unwrap() += 1;
        })
    }
}

/// The detail of a refused credential, or a panic saying what came.
pub fn rejected_detail<T: std::fmt::Debug>(result: SyncResult<T>) -> String {
    match result {
        Err(SyncError::NeedsSignIn(SignInReason::CredentialRejected { detail })) => detail,
        other => panic!("expected a refused sign-in, got {other:?}"),
    }
}

/// The words of a busy server, or a panic saying what came.
pub fn busy_message<T: std::fmt::Debug>(result: SyncResult<T>, service: &str) -> String {
    match result {
        Err(SyncError::Busy {
            service: s,
            message,
            ..
        }) if s == service => message,
        other => panic!("expected a busy {service} server, got {other:?}"),
    }
}
