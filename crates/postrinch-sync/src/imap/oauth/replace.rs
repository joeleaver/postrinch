//! Replacing an IMAP account's secret ([`Replacing`]), as keeping a new
//! grant and adding an account with a password do.
//!
//! The account's access-token slot is held from reading the secret it had
//! until the new one is kept or put back. A refresh holds the same slot
//! while it runs and keeps any refresh token the provider rotated
//! (Microsoft sends a new one with each refresh), so the two cannot
//! interleave: no rotated token is written over the new secret, and the
//! secret read as replaced is the one actually replaced, rotated or not.

use postrinch_model::{EmailAddress, Provider};
use tokio::sync::OwnedMutexGuard;

use super::ImapOAuth;
use super::access::AccessToken;
use super::grant::OAuthGrant;
use crate::imap::ImapSecret;
use crate::{SyncError, SyncResult};

/// An account's secret being replaced: no refresh of its access token
/// runs until this is finished ([`Replacing::finish`]) or undone
/// ([`Replacing::undo`]), or dropped.
pub(crate) struct Replacing<'a> {
    oauth: &'a ImapOAuth,
    account: EmailAddress,
    cached: OwnedMutexGuard<Option<AccessToken>>,
    /// The secret the account had: none when it had none, or one Postrinch
    /// cannot read.
    replaced: Option<ImapSecret>,
    /// Whether the new secret was written over it.
    written: bool,
}

impl ImapOAuth {
    /// Starts replacing `account`'s secret: waits for any refresh of its
    /// access token to finish, then reads the secret it has now. Fails
    /// only when the keychain cannot be read.
    pub(crate) async fn replacing(&self, account: &EmailAddress) -> SyncResult<Replacing<'_>> {
        let cached = self.slot(account).lock_owned().await;
        let replaced = match self.secrets.get(account).await {
            Ok(secret) => Some(secret),
            Err(SyncError::NeedsSignIn(_)) => None,
            Err(e) => return Err(e),
        };
        Ok(Replacing {
            oauth: self,
            account: account.clone(),
            cached,
            replaced,
            written: false,
        })
    }
}

impl Replacing<'_> {
    /// Keeps `secret` as the account's, over the one it had, in the
    /// address's turn and only if it is not a Google account's: one may
    /// have been added since the sign-in started ([`SyncError::AlreadyAdded`],
    /// and then nothing is written).
    pub(crate) async fn write(&mut self, secret: &ImapSecret) -> SyncResult<()> {
        let oauth = self.oauth;
        let account = &self.account;
        let written = &mut self.written;
        let keep = async {
            *written = true;
            oauth.secrets.set(account, secret).await
        };
        oauth.grants.keep_as(account, Provider::Imap, keep).await
    }

    /// The new secret stays. The account's next connections use `grant`'s
    /// access token, when the secret is its grant; otherwise the access
    /// token made with the old secret is forgotten. Returns the secret
    /// that was replaced, for [`ImapOAuth::retire`].
    pub(crate) fn finish(mut self, grant: Option<OAuthGrant>) -> Option<ImapSecret> {
        *self.cached = grant.map(|grant| grant.access);
        self.replaced
    }

    /// Puts back the secret the account had (removing the new one, when
    /// it had none), and leaves its access token as it was. A keychain
    /// that will not take it back is logged: the new secret then stays.
    pub(crate) async fn undo(self) {
        if !self.written {
            return;
        }
        let secrets = &self.oauth.secrets;
        let put_back = match &self.replaced {
            Some(secret) => secrets.set(&self.account, secret).await,
            None => secrets.delete(&self.account).await,
        };
        if let Err(e) = put_back {
            tracing::warn!(
                "could not put {}'s previous sign-in back: {e}",
                self.account
            );
        }
    }
}
