//! One Google grant, two kinds of account. Google accounts (under
//! `postrinch.google`) and Google over IMAP (under `postrinch.imap`) sign
//! in with the same OAuth client, and Google keeps one grant per address
//! and client: revoking any refresh token withdraws it, every token issued
//! under it included. So an address is only ever one kind of account, and
//! a Google token is given back only when the other kind holds none for
//! the address and has no sign-in under way for it (docs/imap.md,
//! "Credentials"). Keeping and revoking take the address's turn
//! ([`GrantLedger`]), so neither decides on a keychain the other is
//! changing.

use std::future::Future;
use std::sync::Arc;

use postrinch_model::{EmailAddress, Provider};
use postrinch_sync_core::{KeychainError, SecretStore};

use super::ledger::{GrantLedger, UnderWay};
use crate::imap::ImapSecrets;
use crate::oauth::OAuthFlow;
use crate::{SyncError, SyncResult};

/// Where both kinds of account keep their Google refresh tokens.
#[derive(Clone)]
pub(crate) struct GoogleGrants {
    google: Arc<dyn SecretStore>,
    imap: ImapSecrets,
}

/// The other kind of account than `kind`.
fn other(kind: Provider) -> Provider {
    match kind {
        Provider::Google => Provider::Imap,
        Provider::Imap => Provider::Google,
    }
}

impl GoogleGrants {
    pub(crate) fn new(google: Arc<dyn SecretStore>, imap: ImapSecrets) -> Self {
        Self { google, imap }
    }

    fn ledger(&self) -> &Arc<GrantLedger> {
        self.imap.ledger()
    }

    /// Counts a sign-in of kind `signing_in` for `address` as under way,
    /// until the guard is dropped: the other kind does not revoke a
    /// Google token for the address meanwhile.
    pub(crate) fn under_way(&self, address: &EmailAddress, signing_in: Provider) -> UnderWay {
        self.ledger().under_way(address, signing_in)
    }

    /// Refuse to add `address` as an account of kind `adding` when the
    /// keychain holds a sign-in for it as the other kind, with
    /// [`SyncError::AlreadyAdded`]. Signing an account of the same kind in
    /// again is not refused.
    pub(crate) async fn refuse_other_kind(
        &self,
        address: &EmailAddress,
        adding: Provider,
    ) -> SyncResult<()> {
        let existing = other(adding);
        let taken = match existing {
            Provider::Imap => self.imap.is_kept(address).await?,
            Provider::Google => self.google_holds(address).await?,
        };
        if taken {
            return Err(SyncError::AlreadyAdded {
                address: address.clone(),
                existing,
            });
        }
        Ok(())
    }

    /// Keeps a sign-in of kind `adding` for `address` with `keep`, in the
    /// address's turn and only once [`Self::refuse_other_kind`] has
    /// passed in it: a sign-in of the other kind that finished while this
    /// one's browser was open is seen, and the two are never both kept.
    pub(crate) async fn keep_as<F>(
        &self,
        address: &EmailAddress,
        adding: Provider,
        keep: F,
    ) -> SyncResult<()>
    where
        F: Future<Output = SyncResult<()>>,
    {
        let _turn = self.ledger().turn(address).await;
        self.refuse_other_kind(address, adding).await?;
        keep.await
    }

    /// Give back a Google refresh token that `holder`'s account for
    /// `address` no longer uses, unless the other kind of account still
    /// holds a Google token for the address, or has a sign-in under way
    /// for it: revoking would withdraw that one too. When the keychain
    /// cannot say, nothing is revoked, as a token left unused only lapses.
    /// A failure to revoke is logged, not returned: the token goes unused
    /// either way.
    pub(crate) async fn revoke_unless_shared(
        &self,
        flow: &OAuthFlow,
        token: &str,
        address: &EmailAddress,
        holder: Provider,
    ) {
        let _turn = self.ledger().turn(address).await;
        let shared = if self.ledger().is_under_way(address, other(holder)) {
            Ok(true)
        } else {
            match holder {
                Provider::Google => self.imap.holds_google_grant(address).await,
                Provider::Imap => self.google_holds(address).await,
            }
        };
        match shared {
            Ok(false) => {
                if let Err(e) = flow.revoke(token).await {
                    tracing::warn!("could not revoke {address}'s refresh token: {e}");
                }
            }
            Ok(true) => {
                tracing::debug!(
                    "{address}'s Google grant is in use or being signed in to; not revoked"
                );
            }
            Err(e) => {
                tracing::warn!(
                    "could not tell whether {address}'s Google grant is still in use, \
                     so it was not revoked: {e}"
                );
            }
        }
    }

    /// Whether a Google account holds a refresh token for `address`.
    async fn google_holds(&self, address: &EmailAddress) -> SyncResult<bool> {
        let google = Arc::clone(&self.google);
        let address = address.clone();
        let held = tokio::task::spawn_blocking(move || google.get(&address))
            .await
            .map_err(|e| {
                SyncError::Keychain(KeychainError::new(format!("keychain task failed: {e}")))
            })??;
        Ok(held.is_some())
    }
}
