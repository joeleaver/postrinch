//! Signing an IMAP account in with OAuth in the browser, again, and out.
//! The grant a sign-in makes, before it is kept, is [`OAuthGrant`].

use postrinch_model::{EmailAddress, Provider, SignInMethod};

use super::access::AccessToken;
use super::grant::OAuthGrant;
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
    /// [`SyncError::OAuthNotSetUp`] before anything opens, and so does an
    /// address that is already a Google account, with
    /// [`SyncError::AlreadyAdded`]: the two kinds of account cannot share
    /// an address, and Google's grant (docs/imap.md, "Credentials").
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
        let grant = self.grant(provider, address, open_browser).await?;
        self.keep(grant).await
    }

    /// [`Self::sign_in`]'s browser sign-in alone: the grant it makes,
    /// kept nowhere yet. Adding an account checks the servers with it
    /// first, then keeps it ([`Self::keep`]) or gives it back
    /// ([`Self::give_back`]), so a failed check leaves the keychain as it
    /// was.
    pub(crate) async fn grant(
        &self,
        provider: OAuthProvider,
        address: &EmailAddress,
        open_browser: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> SyncResult<OAuthGrant> {
        let flow = self.flow(provider)?;
        self.grants
            .refuse_other_kind(address, Provider::Imap)
            .await?;
        // Checked again when the grant is kept (`Replacing::write`): a
        // Google account may be added while the browser is open.
        let under_way = (provider == OAuthProvider::Google)
            .then(|| self.grants.under_way(address, Provider::Imap));
        let Granted {
            tokens,
            refresh_token,
        } = browser_sign_in(&flow, Some(address), open_browser).await?;
        flow.check_scopes(&tokens.scope)?;
        let who = flow.identify(&tokens, Some(address)).await?;
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
        Ok(OAuthGrant {
            provider,
            refresh_token,
            access: AccessToken::from_tokens(&tokens),
            signed_in: OAuthSignIn {
                address: address.clone(),
                method: provider.sign_in_method(),
                display_name: who.name,
            },
            _under_way: under_way,
        })
    }

    /// Keeps a grant as its account's secret, replacing any it had, and
    /// its access token for the account's next connections. No refresh
    /// runs on the old secret meanwhile ([`Replacing`]).
    ///
    /// [`Replacing`]: super::replace::Replacing
    pub(crate) async fn keep(&self, grant: OAuthGrant) -> SyncResult<OAuthSignIn> {
        let mut replacing = self.replacing(&grant.signed_in.address).await?;
        replacing.write(&grant.secret()).await?;
        let signed_in = grant.signed_in.clone();
        replacing.finish(Some(grant));
        Ok(signed_in)
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
    /// (Google, unless a Google account for the address holds a token
    /// under the same grant; Microsoft has no way to), and remove its
    /// secret.
    pub async fn sign_out(&self, account: &EmailAddress) -> SyncResult<()> {
        let slot = self.slot(account);
        let mut cached = slot.lock().await;
        cached.take();
        if let Ok(ImapSecret::OAuth2 {
            provider,
            refresh_token,
        }) = self.secrets.get(account).await
        {
            self.revoke(&provider, &refresh_token, account).await;
        }
        self.secrets.delete(account).await
    }
}
