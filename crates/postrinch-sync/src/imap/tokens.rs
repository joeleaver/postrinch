//! Where OAuth access tokens for IMAP and SMTP come from.
//!
//! Signing in with OAuth needs an access token, which a provider's flow
//! makes from the refresh token kept in the account's [`ImapSecret`]
//! (Google over IMAP, Microsoft; docs/imap.md, "Credentials"). This
//! module only says what IMAP and SMTP need from such a flow, so the
//! flows can be added without touching how servers are spoken to.
//!
//! [`ImapSecret`]: super::ImapSecret

use std::future::Future;
use std::pin::Pin;

use postrinch_model::EmailAddress;

use crate::SyncResult;

/// A boxed future, so [`AccessTokens`] can be used as a trait object.
pub type TokenFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Access tokens for accounts that sign in with OAuth 2.
///
/// An implementation finds the account's grant (its refresh token, in
/// the keychain under `postrinch.imap`, and the provider it names), keeps
/// access tokens until shortly before they expire, and refreshes them
/// with the provider. When a server refuses a token, the caller calls
/// [`AccessTokens::invalidate`] and asks once more; a second refusal is
/// the account's "needs to sign in" state.
pub trait AccessTokens: Send + Sync {
    /// A current access token for `account`. Fails with
    /// [`SyncError::NeedsSignIn`](crate::SyncError::NeedsSignIn) when
    /// there is no grant, or the provider refuses its refresh token, with
    /// the provider's words.
    fn access_token<'a>(&'a self, account: &'a EmailAddress)
    -> TokenFuture<'a, SyncResult<String>>;

    /// Forgets any access token kept for `account`, so the next
    /// [`AccessTokens::access_token`] gets a fresh one.
    fn invalidate<'a>(&'a self, account: &'a EmailAddress) -> TokenFuture<'a, ()>;
}
