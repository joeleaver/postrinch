use postrinch_model::{EmailAddress, SignInReason};
use postrinch_store::StoreError;

pub type SyncResult<T> = Result<T, SyncError>;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The account has to sign in again before anything else can happen.
    /// Callers record this on the account rather than treating it as a
    /// failure.
    #[error("the account must sign in again: {0}")]
    NeedsSignIn(SignInReason),
    #[error("the OAuth client configuration: {0}")]
    Config(String),
    #[error("sign-in failed: {0}")]
    SignIn(String),
    /// This copy of Postrinch has no OAuth client for the provider: its
    /// client file is missing (`detail` is `None`) or does not read. The
    /// provider's accounts cannot sign in; others are unaffected.
    #[error(
        "{provider} sign-in isn't set up in this copy of Postrinch{}",
        .detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default()
    )]
    OAuthNotSetUp {
        provider: String,
        detail: Option<String>,
    },
    /// The browser sign-in was finished by another account than the one
    /// asked for. Nothing was kept.
    #[error("signed in as {signed_in}, not {asked}")]
    OtherAccount {
        asked: EmailAddress,
        signed_in: EmailAddress,
    },
    #[error("the keychain: {0}")]
    Keychain(String),
    #[error("network: {}", crate::google::error_chain(.0))]
    Http(#[from] reqwest::Error),
    /// The provider answered with an error that retrying did not cure.
    #[error("{service} answered {status}: {message}")]
    Api {
        service: &'static str,
        status: u16,
        message: String,
    },
    #[error("a message the provider sent does not parse: {0}")]
    Mime(String),
    /// A mail server could not be reached: the name did not resolve, the
    /// connection was refused, or it did not answer in time.
    #[error("could not reach {server}: {detail}")]
    Unreachable { server: String, detail: String },
    /// The TLS handshake failed: most often a certificate the platform
    /// does not trust for the server's name.
    #[error("the secure connection to {server} failed: {detail}")]
    Tls { server: String, detail: String },
    /// A server offers no encryption (no STARTTLS on a STARTTLS port), so
    /// Postrinch went no further and sent no credentials.
    #[error("{server} offers no secure connection, so Postrinch did not sign in")]
    Insecure { server: String },
    /// A mail server answered with a refusal or spoke out of turn; the
    /// message is the server's own where it gave one.
    #[error("{service} server {server}: {message}")]
    Server {
        service: &'static str,
        server: String,
        message: String,
    },
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl SyncError {
    pub(crate) fn api(service: &'static str, status: u16, message: impl Into<String>) -> Self {
        Self::Api {
            service,
            status,
            message: message.into(),
        }
    }

    /// Whether this is the provider saying the thing asked for is gone.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Api { status: 404, .. })
    }
}
