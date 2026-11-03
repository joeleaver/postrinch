//! Signing in to Google and handing out access tokens.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use postrinch_model::{Account, EmailAddress, Provider, SignInReason};
use tokio::sync::Mutex;

use super::ClientConfig;
use super::loopback::Loopback;
use super::oauth::{Endpoints, OAuth, check_scopes};
use super::pkce::Pkce;
use super::secrets::SecretStore;
use crate::{SyncError, SyncResult};

/// How long the person has to finish signing in in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
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
    oauth: OAuth,
    secrets: Arc<dyn SecretStore>,
    /// Held across a refresh, so concurrent callers wait for one refresh
    /// instead of each starting their own.
    cache: Mutex<HashMap<String, AccessToken>>,
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
            oauth: OAuth {
                config,
                endpoints,
                http: reqwest::Client::new(),
            },
            secrets,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Run the browser sign-in. `open_browser` is given the URL to open;
    /// the call then waits for the person to finish. With `login_hint`, the
    /// account must sign in as that address (signing in again); without it,
    /// any account may (adding one).
    ///
    /// On success the refresh token is in the secret store and the account
    /// is returned, active, for the caller to store. A refresh token the
    /// account had before is revoked.
    pub async fn sign_in(
        &self,
        login_hint: Option<&EmailAddress>,
        open_browser: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> SyncResult<Account> {
        let pkce = Pkce::new();
        let loopback = Loopback::bind().await?;
        let redirect_uri = loopback.redirect_uri().to_owned();
        let url =
            self.oauth
                .authorize_url(&redirect_uri, &pkce.challenge, &pkce.state, login_hint)?;
        open_browser(url.as_str())?;

        let code = loopback.wait_for_code(&pkce.state, SIGN_IN_TIMEOUT).await?;
        let tokens = self
            .oauth
            .exchange_code(&code, &pkce.verifier, &redirect_uri)
            .await?;
        let refresh_token = tokens
            .refresh_token
            .clone()
            .ok_or_else(|| SyncError::SignIn("Google sent no refresh token".into()))?;
        // A grant without what Postrinch needs is useless: give its token
        // back rather than leave it counting against Google's limit.
        if let Err(e) = check_scopes(&tokens.scope) {
            self.revoke_quietly(&refresh_token).await;
            return Err(e);
        }

        let user = self.oauth.userinfo(&tokens.access_token).await?;
        if !user.email_verified {
            return Err(SyncError::SignIn(format!(
                "{} is not a verified address",
                user.email
            )));
        }
        let address =
            EmailAddress::parse(&user.email).map_err(|e| SyncError::SignIn(e.to_string()))?;
        if let Some(hint) = login_hint
            && hint.key() != address.key()
        {
            self.revoke_quietly(&refresh_token).await;
            return Err(SyncError::SignIn(format!(
                "signed in as {address}, not {hint}"
            )));
        }

        // A token that cannot be kept is given back, as above.
        let stored = refresh_token.clone();
        let previous = match self
            .secret(&address, move |s, a| {
                let previous = s.get(a)?;
                s.set(a, &stored)?;
                Ok(previous)
            })
            .await
        {
            Ok(previous) => previous,
            Err(e) => {
                self.revoke_quietly(&refresh_token).await;
                return Err(e);
            }
        };
        if let Some(previous) = previous.filter(|p| *p != refresh_token) {
            self.revoke_quietly(&previous).await;
        }

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

    /// Revoke a token Postrinch is done with. A failure is logged, not
    /// returned: the token is unused either way, and Google retires the
    /// oldest when an account has too many.
    async fn revoke_quietly(&self, token: &str) {
        if let Err(e) = self.oauth.revoke(token).await {
            tracing::warn!("could not revoke an old refresh token: {e}");
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
