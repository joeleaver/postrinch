//! Signing in to Google and handing out access tokens.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use postrinch_model::{Account, EmailAddress, Provider, SignInReason};
use tokio::sync::Mutex;

use super::ClientConfig;
use super::oauth::{Endpoints, api_spec, check_scopes};
use crate::oauth::{Granted, OAuthFlow, browser_sign_in};
use crate::secrets::SecretStore;
use crate::{SyncError, SyncResult};

/// Refresh an access token this long before Google says it expires.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

struct AccessToken {
    token: String,
    expires: Instant,
}

/// Google sign-in for any number of accounts. Refresh tokens live in a
/// [`SecretStore`]; access tokens are cached in memory until shortly before
/// they expire. Cheap to share: wrap in an `Arc`.
pub struct GoogleAuth {
    oauth: OAuthFlow,
    secrets: Arc<dyn SecretStore>,
    /// Held across a refresh, so concurrent callers wait for one refresh
    /// instead of each starting their own.
    cache: Mutex<HashMap<String, AccessToken>>,
    /// The scopes each account's grant has, as Google last said, by
    /// account key.
    scopes: std::sync::Mutex<HashMap<String, String>>,
}

impl GoogleAuth {
    pub fn new(config: ClientConfig, secrets: Arc<dyn SecretStore>) -> Self {
        let endpoints = Endpoints::google(&config);
        Self::with_endpoints(config, endpoints, secrets)
    }

    pub fn with_endpoints(
        config: ClientConfig,
        endpoints: Endpoints,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            oauth: OAuthFlow::new(api_spec(&config, endpoints), reqwest::Client::new()),
            secrets,
            cache: Mutex::new(HashMap::new()),
            scopes: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Run the browser sign-in. `open_browser` is given the URL to open;
    /// the call then waits for the person to finish. With `login_hint`, the
    /// account must sign in as that address (signing in again); without it,
    /// any account may (adding one).
    ///
    /// On success the refresh token is in the secret store, replacing any
    /// the account had, and the account is returned, active, for the caller
    /// to store.
    pub async fn sign_in(
        &self,
        login_hint: Option<&EmailAddress>,
        open_browser: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> SyncResult<Account> {
        let Granted {
            tokens,
            refresh_token,
        } = browser_sign_in(&self.oauth, login_hint, open_browser).await?;

        let user = self.oauth.userinfo(&tokens.access_token).await?;
        let address =
            EmailAddress::parse(&user.email).map_err(|e| SyncError::SignIn(e.to_string()))?;
        // Revoking a Google refresh token withdraws the whole grant for the
        // account and this client, every token issued under it included. So
        // a token Postrinch cannot use is given back only when Postrinch
        // holds no other token for the account; otherwise it is left to
        // lapse unused, and the token the account already has keeps working.
        let held = self.secret(&address, |s, a| s.get(a)).await?.is_some();

        let refused = if let Err(e) = check_scopes(&tokens.scope) {
            Some(e)
        } else if !user.email_verified {
            Some(SyncError::SignIn(format!(
                "{address} is not a verified address"
            )))
        } else if let Some(hint) = login_hint
            && hint.key() != address.key()
        {
            Some(SyncError::SignIn(format!(
                "signed in as {address}, not {hint}"
            )))
        } else {
            None
        };
        if let Some(e) = refused {
            if !held {
                self.revoke_quietly(&refresh_token).await;
            }
            return Err(e);
        }

        // The new token replaces the one the account had, which shares its
        // grant: neither is revoked.
        let stored = refresh_token.clone();
        if let Err(e) = self.secret(&address, move |s, a| s.set(a, &stored)).await {
            if !held {
                self.revoke_quietly(&refresh_token).await;
            }
            return Err(e);
        }

        self.note_scopes(&address, &tokens.scope);
        self.cache.lock().await.insert(
            address.key(),
            AccessToken {
                token: tokens.access_token,
                expires: Instant::now() + Duration::from_secs(tokens.expires_in),
            },
        );

        let mut account = Account::new(address, Provider::Google);
        account.display_name = user.name.filter(|n| !n.trim().is_empty());
        Ok(account)
    }

    /// A current access token for `account`, refreshed if needed. Fails
    /// with [`SyncError::NeedsSignIn`] when the account has no refresh token
    /// or Google no longer accepts it.
    pub async fn access_token(&self, account: &EmailAddress) -> SyncResult<String> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.get(&account.key())
            && cached.expires > Instant::now() + EXPIRY_MARGIN
        {
            return Ok(cached.token.clone());
        }
        let refresh_token = self
            .secret(account, |s, a| s.get(a))
            .await?
            .ok_or(SyncError::NeedsSignIn(SignInReason::NoCredential))?;
        let tokens = self.oauth.refresh(&refresh_token).await?;
        self.note_scopes(account, &tokens.scope);
        let token = tokens.access_token.clone();
        cache.insert(
            account.key(),
            AccessToken {
                token: tokens.access_token,
                expires: Instant::now() + Duration::from_secs(tokens.expires_in),
            },
        );
        Ok(token)
    }

    fn note_scopes(&self, account: &EmailAddress, scopes: &str) {
        if !scopes.is_empty() {
            self.scopes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(account.key(), scopes.to_owned());
        }
    }

    /// Whether the account's grant has `scope`. When Google has not said
    /// which scopes it has, it is taken to: a call that needs it is then
    /// refused, and says so.
    pub async fn has_scope(&self, account: &EmailAddress, scope: &str) -> SyncResult<bool> {
        self.access_token(account).await?;
        let scopes = self.scopes.lock().unwrap_or_else(|e| e.into_inner());
        Ok(scopes
            .get(&account.key())
            .is_none_or(|s| s.split_whitespace().any(|g| g == scope)))
    }

    /// Forget the cached access token, as when an API call is answered 401.
    pub async fn invalidate(&self, account: &EmailAddress) {
        self.cache.lock().await.remove(&account.key());
    }

    /// Sign an account out: revoke its refresh token and remove it from the
    /// secret store.
    pub async fn sign_out(&self, account: &EmailAddress) -> SyncResult<()> {
        self.invalidate(account).await;
        if let Some(token) = self.secret(account, |s, a| s.get(a)).await? {
            self.revoke_quietly(&token).await;
        }
        self.secret(account, |s, a| s.delete(a)).await
    }

    /// Revoke a token, and with it the account's whole grant to Postrinch.
    /// A failure is logged, not returned: the token goes unused either way,
    /// and Google retires the oldest when an account has too many.
    async fn revoke_quietly(&self, token: &str) {
        if let Err(e) = self.oauth.revoke(token).await {
            tracing::warn!("could not revoke a refresh token: {e}");
        }
    }

    /// Run a secret-store call on a blocking thread.
    async fn secret<T: Send + 'static>(
        &self,
        account: &EmailAddress,
        call: impl FnOnce(&dyn SecretStore, &EmailAddress) -> SyncResult<T> + Send + 'static,
    ) -> SyncResult<T> {
        let secrets = Arc::clone(&self.secrets);
        let account = account.clone();
        tokio::task::spawn_blocking(move || call(secrets.as_ref(), &account))
            .await
            .map_err(|e| SyncError::Keychain(format!("keychain task failed: {e}")))?
    }
}
