//! A browser sign-in's grant before it is kept ([`OAuthGrant`]), and
//! giving grants back: one a failed check leaves unused, and one a new
//! sign-in replaced (docs/imap.md, "Adding an account").
//!
//! With Google, every token a client was given for an address is one
//! grant: revoking any of them withdraws them all. So a token is given
//! back only when no sign-in kept here for the address may share its
//! grant: not the IMAP account's own secret, when it is a grant with the
//! same provider, and not a Google account's ([`GoogleGrants`]).
//!
//! [`GoogleGrants`]: crate::google::GoogleGrants

use std::fmt;

use postrinch_model::{EmailAddress, Provider, SignInMethod, SignInReason};

use super::access::AccessToken;
use super::sign_in::OAuthSignIn;
use super::{ImapOAuth, OAuthProvider};
use crate::imap::{AccessTokens, ImapSecret, TokenFuture};
use crate::{SyncError, SyncResult};

/// A finished browser sign-in whose grant is kept nowhere yet: the
/// servers are checked with it ([`AccessTokens`] for its own account
/// alone), and then it is kept ([`ImapOAuth::keep`]) or given back
/// ([`ImapOAuth::give_back`]).
pub(crate) struct OAuthGrant {
    pub(super) provider: OAuthProvider,
    pub(super) refresh_token: String,
    pub(super) access: AccessToken,
    pub(super) signed_in: OAuthSignIn,
}

impl OAuthGrant {
    /// The account's [`ImapSettings::sign_in`](postrinch_model::ImapSettings).
    pub(crate) fn method(&self) -> SignInMethod {
        self.signed_in.method.clone()
    }

    /// The name the provider has for the person, when it gave one.
    pub(crate) fn display_name(&self) -> Option<String> {
        self.signed_in.display_name.clone()
    }

    /// The secret it is kept as.
    pub(crate) fn secret(&self) -> ImapSecret {
        ImapSecret::OAuth2 {
            provider: self.provider.id().to_owned(),
            refresh_token: self.refresh_token.clone(),
        }
    }
}

/// Never shows its tokens.
impl fmt::Debug for OAuthGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthGrant")
            .field("provider", &self.provider)
            .field("address", &self.signed_in.address)
            .finish_non_exhaustive()
    }
}

/// The sign-in's own access token, for its own account. It was made
/// moments ago, so there is nothing to refresh: a server that refuses it
/// refuses the grant.
impl AccessTokens for OAuthGrant {
    fn access_token<'a>(
        &'a self,
        account: &'a EmailAddress,
    ) -> TokenFuture<'a, SyncResult<String>> {
        let token = if account.key() == self.signed_in.address.key() {
            Ok(self.access.token().to_owned())
        } else {
            Err(SyncError::NeedsSignIn(SignInReason::NoCredential))
        };
        Box::pin(std::future::ready(token))
    }

    fn invalidate<'a>(&'a self, _account: &'a EmailAddress) -> TokenFuture<'a, ()> {
        Box::pin(std::future::ready(()))
    }
}

impl ImapOAuth {
    /// Gives back a grant that is not to be kept (the servers did not
    /// take it), unless the account's secret here is a grant with the
    /// same provider, which may be the same grant. When the keychain
    /// cannot say, it is left to lapse.
    pub(crate) async fn give_back(&self, grant: OAuthGrant) {
        let address = &grant.signed_in.address;
        match self.secrets.get(address).await {
            Ok(ImapSecret::OAuth2 { provider, .. }) if provider == grant.provider.id() => {
                tracing::debug!("{address}'s kept grant may be the unused one's; not given back");
                return;
            }
            Ok(_) | Err(SyncError::NeedsSignIn(_)) => {}
            Err(e) => {
                tracing::warn!(
                    "could not tell whether {address}'s unused grant is still held, \
                     so it was not given back: {e}"
                );
                return;
            }
        }
        self.revoke(grant.provider.id(), &grant.refresh_token, address)
            .await;
    }

    /// The account's secret was `replaced` by `kept`: gives the replaced
    /// grant back, if it was one, unless `kept` is a grant with the same
    /// provider (with Google, the same grant: revoking the old token would
    /// withdraw the new one). An account that now signs in with a
    /// password has its access token forgotten too.
    pub(crate) async fn retire(
        &self,
        account: &EmailAddress,
        replaced: &ImapSecret,
        kept: &ImapSecret,
    ) {
        let ImapSecret::OAuth2 {
            provider,
            refresh_token,
        } = replaced
        else {
            return;
        };
        match kept {
            ImapSecret::OAuth2 { provider: now, .. } if now == provider => return,
            ImapSecret::OAuth2 { .. } => {}
            ImapSecret::Password { .. } => {
                self.slot(account).lock().await.take();
            }
        }
        self.revoke(provider, refresh_token, account).await;
    }

    /// Gives a refresh token back to its provider where the provider takes
    /// it back (Google, unless a Google account for the address holds a
    /// token under the same grant; Microsoft has no way to). A failure is
    /// logged, not returned: the token goes unused either way.
    pub(super) async fn revoke(&self, provider: &str, refresh_token: &str, account: &EmailAddress) {
        let Some(provider) = OAuthProvider::from_id(provider) else {
            return;
        };
        let Ok(flow) = self.flow(provider) else {
            return;
        };
        match provider {
            OAuthProvider::Google => {
                self.grants
                    .revoke_unless_shared(&flow, refresh_token, account, Provider::Imap)
                    .await;
            }
            OAuthProvider::Microsoft => {
                if let Err(e) = flow.revoke(refresh_token).await {
                    tracing::warn!("could not revoke {account}'s refresh token: {e}");
                }
            }
        }
    }
}
