//! Checking a new account's settings before it is stored (docs/imap.md,
//! "Adding an account", step 4): sign in to IMAP and list the folders,
//! sign in to SMTP (EHLO and AUTH, no mail sent). Each server's outcome is
//! its own, so the dialog can say which failed and what it said, and open
//! the manual settings with the rest filled in.

use postrinch_model::{EmailAddress, ImapSettings, SignInReason};

use super::connect::Connector;
use super::folders::ListedFolder;
use super::secret::ImapSecret;
use super::session::ImapSession;
use super::smtp::SmtpConnection;
use super::tokens::AccessTokens;
use crate::SyncError;

/// How the check went, server by server.
#[derive(Debug)]
pub struct AccountCheck {
    /// The folders, when IMAP signed in and listed them.
    pub imap: Result<Vec<ListedFolder>, CheckFailure>,
    /// Whether SMTP signed in.
    pub smtp: Result<(), CheckFailure>,
}

impl AccountCheck {
    /// Whether both servers passed.
    pub fn is_ok(&self) -> bool {
        self.imap.is_ok() && self.smtp.is_ok()
    }
}

/// Why one server failed the check, as the dialog tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckFailure {
    /// It could not be reached: the name, the port, a refused or silent
    /// connection.
    Unreachable(String),
    /// The secure connection failed, often a certificate not trusted for
    /// the name.
    Tls(String),
    /// It offers no encryption, so no password was sent.
    Insecure,
    /// It refused the sign-in, in its own words.
    SignIn(SignInReason),
    /// It turned the account away for now (too many connections, a
    /// limit, its sign-in service down), in its own words: the settings
    /// may well be right, and trying again later may pass.
    Busy(String),
    /// It answered something else, in its own words where it gave them.
    Server(String),
}

impl From<SyncError> for CheckFailure {
    fn from(error: SyncError) -> Self {
        match error {
            SyncError::Unreachable { detail, .. } => Self::Unreachable(detail),
            SyncError::Tls { detail, .. } => Self::Tls(detail),
            SyncError::Insecure { .. } => Self::Insecure,
            SyncError::NeedsSignIn(reason) => Self::SignIn(reason),
            SyncError::Busy { message, .. } => Self::Busy(message),
            SyncError::Server { message, .. } => Self::Server(message),
            other => Self::Server(other.to_string()),
        }
    }
}

/// Checks both servers of an account at once, with the secret the person
/// gave (not yet kept anywhere). For an OAuth account, `tokens` has the
/// grant the sign-in just made.
pub async fn check_account(
    connector: &Connector,
    account: &EmailAddress,
    settings: &ImapSettings,
    secret: &ImapSecret,
    tokens: &dyn AccessTokens,
) -> AccountCheck {
    let imap = async {
        let mut session = ImapSession::open(connector, account, settings, secret, tokens).await?;
        let folders = session.list_folders().await?;
        // The settings are proven once the folders are listed: a server
        // that drops the connection at LOGOUT has not failed the check.
        if let Err(e) = session.logout().await {
            tracing::debug!("IMAP check: signing out after listing the folders failed: {e}");
        }
        Ok::<_, SyncError>(folders)
    };
    let smtp = async {
        SmtpConnection::open(connector, account, settings, secret, tokens)
            .await?
            .quit()
            .await
    };
    let (imap, smtp) = tokio::join!(imap, smtp);
    AccountCheck {
        imap: imap.map_err(CheckFailure::from),
        smtp: smtp.map_err(CheckFailure::from),
    }
}
