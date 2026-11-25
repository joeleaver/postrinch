use postrinch_model::{EmailAddress, Provider, SignInReason};
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
    /// The person stopped a browser sign-in while it waited for the
    /// browser ([`crate::imap::SignInCanceller`]). Nothing was kept.
    #[error("the sign-in was cancelled")]
    SignInCancelled,
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
    /// The address is already here as the other kind of account (a
    /// Google account, or an IMAP account): the two cannot both be kept,
    /// so nothing was added (docs/imap.md, "Credentials").
    #[error("{address} is already added as {}", kind_phrase(*.existing))]
    AlreadyAdded {
        address: EmailAddress,
        existing: Provider,
    },
    /// A calendar call for an IMAP account: calendars are Google
    /// accounts' only (docs/imap.md, "The goal").
    #[error("{address} is an IMAP account, and calendars are Google accounts' only")]
    NoCalendars { address: EmailAddress },
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
    /// A mail server turned the account away for now: too many
    /// connections, a bandwidth or command limit, a sign-in service that
    /// is down. Nothing is wrong with the credential, so the account is
    /// not asked to sign in and no fresh token is fetched; it is to be
    /// tried again later, as [`Backoff`](crate::Backoff) says (the IMAP
    /// keep-in-sync and sending loops do, and send
    /// [`SyncEvent::Busy`](crate::SyncEvent::Busy); the account check
    /// reports it as `CheckFailure::Busy`). The message is the server's
    /// own sentence.
    #[error("{service} server {server} is busy: {message}")]
    Busy {
        service: &'static str,
        server: String,
        message: String,
        /// Whether the IMAP server said the account has too many
        /// connections open (`[LIMIT]` at sign-in, or a known
        /// connection limit's words: `imap::refusal::names_connections`).
        /// An IMAP account's IDLE connection then gives way to the
        /// others rather than waiting the account out (docs/imap.md,
        /// "Sync"). Always `false` for SMTP.
        connections: bool,
    },
    /// A server would not search as asked (`NO [BADCHARSET]` to text that
    /// is not ASCII, a term IMAP cannot ask, a `SEARCH` refused in every
    /// folder): the search keeps to the mail here. The message says why,
    /// in the server's words where it gave some.
    #[error("{server} could not search: {message}")]
    SearchRefused { server: String, message: String },
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// "a Google account", "an IMAP account".
fn kind_phrase(provider: Provider) -> &'static str {
    match provider {
        Provider::Google => "a Google account",
        Provider::Imap => "an IMAP account",
    }
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
