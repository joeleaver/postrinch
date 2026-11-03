use postrinch_model::SignInReason;
use postrinch_store::StoreError;

pub type SyncResult<T> = Result<T, SyncError>;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The account has to sign in again before anything else can happen.
    /// Callers record this on the account rather than treating it as a
    /// failure.
    #[error("the account must sign in again: {0:?}")]
    NeedsSignIn(SignInReason),
    #[error("the OAuth client configuration: {0}")]
    Config(String),
    #[error("sign-in failed: {0}")]
    SignIn(String),
    #[error("the keychain: {0}")]
    Keychain(String),
    #[error("network: {0}")]
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
