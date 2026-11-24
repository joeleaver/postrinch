//! Signing an IMAP account in with OAuth in the browser, again, and out.

use postrinch_model::{EmailAddress, SignInMethod};

use super::access::AccessToken;
use super::{ImapOAuth, OAuthProvider};
use crate::imap::ImapSecret;
use crate::oauth::{Granted, browser_sign_in};
use crate::{SyncError, SyncResult};

/// A finished OAuth sign-in: what the account's record needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthSignIn {
    /// The address that signed in: the one asked for.
    pub address: EmailAddress,
    /// The account's [`ImapSettings::sign_in`](postrinch_model::ImapSettings).
    pub method: SignInMethod,
    /// The name the provider has for the person, when it gave one.
    pub display_name: Option<String>,
}

impl ImapOAuth {
    /// Sign `address` in with `provider` in the browser: `open_browser` is
    /// handed the provider's page, and the call waits for the person to
    /// finish. The address is the login hint, and the sign-in must be
    /// for it: another account's is refused with
    /// [`SyncError::OtherAccount`], naming the one that signed in. On
    /// success the account's secret holds the new refresh token, replacing
    /// any it had.
    ///
    /// A provider without a client here fails with
    /// [`SyncError::OAuthNotSetUp`] before anything opens.
    ///
    /// A refused sign-in's token is not revoked but left to lapse: with
    /// Google, revoking it would withdraw the whole grant for the address,
    /// and with it a Google account's sign-in if the address has one.
    pub async fn sign_in(
        &self,
        provider: OAuthProvider,
        address: &EmailAddress,
        open_browser: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> SyncResult<OAuthSignIn> {
        let flow = self.flow(provider)?;
        let Granted {
            tokens,
            refresh_token,
        } = browser_sign_in(&flow, Some(address), open_browser).await?;
        flow.check_scopes(&tokens.scope)?;
        let who = flow.identify(&tokens).await?;
        if who.address.key() != address.key() {
            return Err(SyncError::OtherAccount {
                asked: address.clone(),
                signed_in: who.address,
            });
        }
        if !who.verified {
            return Err(SyncError::SignIn(format!(
                "{address} is not a verified address"
            )));
        }

        // Held while the secret changes, so no refresh runs on the old one
        // meanwhile and caches what it got after the new token is in.
        let slot = self.slot(address);
        let mut cached = slot.lock().await;
        let secret = ImapSecret::OAuth2 {
            provider: provider.id().to_owned(),
            refresh_token,
        };
        self.secrets.set(address, &secret).await?;
        *cached = Some(AccessToken::from_tokens(&tokens));

        Ok(OAuthSignIn {
            address: address.clone(),
            method: provider.sign_in_method(),
            display_name: who.name,
        })
    }

    /// Sign an existing account in again, as its settings say: "Sign in
    /// again" on an account that needs to. `method` is the account's
    /// [`SignInMethod`]; one that is not OAuth's has no browser sign-in.
    pub async fn sign_in_again(
        &self,
        account: &EmailAddress,
        method: &SignInMethod,
        open_browser: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> SyncResult<OAuthSignIn> {
        let SignInMethod::OAuth2 { provider } = method else {
            return Err(SyncError::SignIn(format!(
                "{account} signs in with a password, not in the browser"
            )));
        };
        let provider =
            OAuthProvider::from_id(provider).ok_or_else(|| SyncError::OAuthNotSetUp {
                provider: provider.clone(),
                detail: None,
            })?;
        self.sign_in(provider, account, open_browser).await
    }

    /// Sign an account out: forget its access token, give its refresh
    /// token back to the provider where the provider takes it back
    /// (Google; Microsoft has no way to), and remove its secret.
    pub async fn sign_out(&self, account: &EmailAddress) -> SyncResult<()> {
        let slot = self.slot(account);
        let mut cached = slot.lock().await;
        cached.take();
        if let Ok(ImapSecret::OAuth2 {
            provider,
            refresh_token,
        }) = self.secrets.get(account).await
            && let Some(flow) = OAuthProvider::from_id(&provider).and_then(|p| self.flow(p).ok())
            && let Err(e) = flow.revoke(&refresh_token).await
        {
            // The token goes unused either way.
            tracing::warn!("could not revoke {account}'s refresh token: {e}");
        }
        self.secrets.delete(account).await
    }
}
