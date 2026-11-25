//! Access tokens for IMAP accounts that sign in with OAuth: made from the
//! refresh token in the account's [`ImapSecret`], kept until shortly
//! before they expire.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use postrinch_model::{EmailAddress, SignInReason};
use tokio::sync::Mutex;

use super::{OAuthClients, OAuthProvider};
use crate::google::GoogleGrants;
use crate::imap::{AccessTokens, ImapSecret, ImapSecrets, TokenFuture};
use crate::oauth::{OAuthFlow, OAuthTimeouts, Tokens, http_client};
use crate::secrets::SecretStore;
use crate::{SyncError, SyncResult};

/// Refresh an access token this long before the provider says it expires.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

pub(super) struct AccessToken {
    token: String,
    expires: Instant,
}

impl AccessToken {
    pub(super) fn from_tokens(tokens: &Tokens) -> Self {
        Self {
            token: tokens.access_token.clone(),
            expires: Instant::now() + Duration::from_secs(tokens.expires_in),
        }
    }

    fn fresh(&self) -> Option<&str> {
        (self.expires > Instant::now() + EXPIRY_MARGIN).then_some(self.token.as_str())
    }
}

/// One account's access token. Its lock is held across a refresh, so
/// concurrent callers for the account wait for one refresh rather than
/// each starting their own; other accounts are not held up.
pub(super) type Slot = Arc<Mutex<Option<AccessToken>>>;

/// OAuth for IMAP accounts: sign-in through the browser
/// ([`ImapOAuth::sign_in`]) and the access tokens IMAP and SMTP sign in
/// with ([`AccessTokens`]). Refresh tokens are the accounts' own, in
/// [`ImapSecrets`] (`postrinch.imap`), apart from any Google account's,
/// which is read, never changed: an address that is a Google account is
/// not added as an IMAP account, and a grant it shares is not revoked.
/// Cheap to share: wrap in an `Arc`.
pub struct ImapOAuth {
    pub(super) clients: OAuthClients,
    pub(super) secrets: ImapSecrets,
    pub(super) grants: GoogleGrants,
    http: reqwest::Client,
    slots: std::sync::Mutex<HashMap<String, Slot>>,
}

impl ImapOAuth {
    /// IMAP accounts' secrets in `secrets`, Google accounts' refresh
    /// tokens in `google_secrets` (the keychain's `postrinch.google`).
    pub fn new(
        clients: OAuthClients,
        secrets: ImapSecrets,
        google_secrets: Arc<dyn SecretStore>,
    ) -> SyncResult<Self> {
        Ok(Self {
            clients,
            grants: GoogleGrants::new(google_secrets, secrets.clone()),
            secrets,
            http: http_client(OAuthTimeouts::default())?,
            slots: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// With other timeouts for the providers' OAuth requests than the
    /// default.
    pub fn with_timeouts(mut self, timeouts: OAuthTimeouts) -> SyncResult<Self> {
        self.http = http_client(timeouts)?;
        Ok(self)
    }

    /// Whether accounts with `provider` can sign in in this copy of
    /// Postrinch.
    pub fn is_set_up(&self, provider: OAuthProvider) -> bool {
        self.clients.is_set_up(provider)
    }

    pub(super) fn flow(&self, provider: OAuthProvider) -> SyncResult<OAuthFlow> {
        let spec = self.clients.spec(provider)?.clone();
        Ok(OAuthFlow::new(spec, self.http.clone()))
    }

    pub(super) fn slot(&self, account: &EmailAddress) -> Slot {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(slots.entry(account.key()).or_default())
    }

    async fn current_token(&self, account: &EmailAddress) -> SyncResult<String> {
        let slot = self.slot(account);
        let mut cached = slot.lock().await;
        if let Some(token) = cached.as_ref().and_then(AccessToken::fresh) {
            return Ok(token.to_owned());
        }
        let ImapSecret::OAuth2 {
            provider: provider_id,
            refresh_token,
        } = self.secrets.get(account).await?
        else {
            return Err(SyncError::NeedsSignIn(SignInReason::NoCredential));
        };
        // A provider a later Postrinch added, whose grant this one cannot use.
        let provider =
            OAuthProvider::from_id(&provider_id).ok_or_else(|| SyncError::OAuthNotSetUp {
                provider: provider_id.clone(),
                detail: None,
            })?;
        let tokens = self.flow(provider)?.refresh(&refresh_token).await?;
        if let Some(rotated) = tokens.refresh_token.as_deref()
            && rotated != refresh_token
        {
            self.keep_rotated(account, provider, rotated).await;
        }
        let fresh = AccessToken::from_tokens(&tokens);
        let token = fresh.token.clone();
        *cached = Some(fresh);
        Ok(token)
    }

    /// Keep the refresh token a provider rotated in (Microsoft sends a new
    /// one with each refresh). If the keychain will not take it, the old
    /// one is kept: Microsoft does not retire a refresh token when it
    /// issues the next, so it still works until it expires.
    async fn keep_rotated(&self, account: &EmailAddress, provider: OAuthProvider, rotated: &str) {
        let secret = ImapSecret::OAuth2 {
            provider: provider.id().to_owned(),
            refresh_token: rotated.to_owned(),
        };
        if let Err(e) = self.secrets.set(account, &secret).await {
            tracing::warn!("could not keep {account}'s new refresh token: {e}");
        }
    }
}

impl AccessTokens for ImapOAuth {
    fn access_token<'a>(
        &'a self,
        account: &'a EmailAddress,
    ) -> TokenFuture<'a, SyncResult<String>> {
        Box::pin(self.current_token(account))
    }

    fn invalidate<'a>(&'a self, account: &'a EmailAddress) -> TokenFuture<'a, ()> {
        Box::pin(async move {
            self.slot(account).lock().await.take();
        })
    }
}
