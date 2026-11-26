//! Signing in to Google and handing out access tokens.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use postrinch_model::{Account, EmailAddress, Provider, SignInReason};
use postrinch_sync_core::{KeychainError, KeychainResult, SecretStore};
use tokio::sync::Mutex;

use super::ClientConfig;
use super::grant::GoogleGrants;
use super::oauth::{Endpoints, api_spec, check_scopes};
use crate::imap::ImapSecrets;
use crate::oauth::{Granted, OAuthFlow, OAuthTimeouts, browser_sign_in, http_client};
use crate::{SyncError, SyncResult};

/// Refresh an access token this long before Google says it expires.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

struct AccessToken {
    token: String,
    expires: Instant,
}

/// Google sign-in for any number of accounts. Refresh tokens live in a
/// [`SecretStore`]; access tokens are cached in memory until shortly before
/// they expire. IMAP accounts' secrets are read too, never changed: an
/// address that is an IMAP account is not added as a Google account, and a
/// grant an IMAP account shares is not revoked. Cheap to share: wrap in an
/// `Arc`.
pub struct GoogleAuth {
    oauth: OAuthFlow,
    secrets: Arc<dyn SecretStore>,
    grants: GoogleGrants,
    /// Held across a refresh, so concurrent callers wait for one refresh
    /// instead of each starting their own.
    cache: Mutex<HashMap<String, AccessToken>>,
    /// The scopes each account's grant has, as Google last said, by
    /// account key.
    scopes: std::sync::Mutex<HashMap<String, String>>,
}

impl GoogleAuth {
    /// Google accounts' refresh tokens in `secrets` (the keychain's
    /// `postrinch.google`), IMAP accounts' secrets in `imap_secrets`.
    pub fn new(
        config: ClientConfig,
        secrets: Arc<dyn SecretStore>,
        imap_secrets: ImapSecrets,
    ) -> SyncResult<Self> {
        let endpoints = Endpoints::google(&config);
        Self::with_endpoints(config, endpoints, secrets, imap_secrets)
    }

    pub fn with_endpoints(
        config: ClientConfig,
        endpoints: Endpoints,
        secrets: Arc<dyn SecretStore>,
        imap_secrets: ImapSecrets,
    ) -> SyncResult<Self> {
        let http = http_client(OAuthTimeouts::default())?;
        Ok(Self {
            oauth: OAuthFlow::new(api_spec(&config, endpoints), http),
            grants: GoogleGrants::new(Arc::clone(&secrets), imap_secrets),
            secrets,
            cache: Mutex::new(HashMap::new()),
            scopes: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// With other timeouts for Google's OAuth requests than the default.
    pub fn with_timeouts(mut self, timeouts: OAuthTimeouts) -> SyncResult<Self> {
        self.oauth.http = http_client(timeouts)?;
        Ok(self)
    }

    /// Run the browser sign-in. `open_browser` is given the URL to open;
    /// the call then waits for the person to finish. With `login_hint`, the
    /// account must sign in as that address (signing in again); without it,
    /// any account may (adding one).
    ///
    /// On success the refresh token is in the secret store, replacing any
    /// the account had, and the account is returned, active, for the caller
    /// to store.
    ///
    /// An address that is already an IMAP account is refused with
    /// [`SyncError::AlreadyAdded`]: before the browser opens when it is the
    /// login hint, otherwise once Google says who signed in.
    pub async fn sign_in(
        &self,
        login_hint: Option<&EmailAddress>,
        open_browser: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> SyncResult<Account> {
        // Counted as under way from before the browser opens when the
        // address is known, otherwise from when Google names it: an IMAP
        // account's give-back does not revoke meanwhile.
        let mut under_way = None;
        if let Some(hint) = login_hint {
            self.grants
                .refuse_other_kind(hint, Provider::Google)
                .await?;
            under_way = Some(self.grants.under_way(hint, Provider::Google));
        }
        let Granted {
            tokens,
            refresh_token,
        } = browser_sign_in(&self.oauth, login_hint, open_browser).await?;

        let user = self.oauth.userinfo(&tokens.access_token).await?;
        let address =
            EmailAddress::parse(&user.email).map_err(|e| SyncError::SignIn(e.to_string()))?;
        let _under_way =
            under_way.unwrap_or_else(|| self.grants.under_way(&address, Provider::Google));
        // Revoking a Google refresh token withdraws the whole grant for the
        // account and this client, every token issued under it included. So
        // a token Postrinch cannot use is given back only when Postrinch
        // holds no other token for the account (here, or an IMAP account's:
        // `revoke_unless_shared`); otherwise it is left to lapse unused, and
        // the token the account already has keeps working.
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
            self.grants
                .refuse_other_kind(&address, Provider::Google)
                .await
                .err()
        };
        if let Some(e) = refused {
            if !held {
                self.revoke_unless_shared(&refresh_token, &address).await;
            }
            return Err(e);
        }

        // The new token replaces the one the account had, which shares its
        // grant: neither is revoked. It is kept only if no IMAP account
        // was added for the address while the browser was open.
        let stored = refresh_token.clone();
        let keep = self.secret(&address, move |s, a| s.set(a, &stored));
        if let Err(e) = self.grants.keep_as(&address, Provider::Google, keep).await {
            if !held {
                self.revoke_unless_shared(&refresh_token, &address).await;
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

    /// Sign an account out: revoke its refresh token, unless an IMAP
    /// account shares its grant, and remove it from the secret store.
    pub async fn sign_out(&self, account: &EmailAddress) -> SyncResult<()> {
        self.invalidate(account).await;
        if let Some(token) = self.secret(account, |s, a| s.get(a)).await? {
            self.revoke_unless_shared(&token, account).await;
        }
        self.secret(account, |s, a| s.delete(a)).await
    }

    /// Revoke a token, and with it the address's whole grant to Postrinch,
    /// unless an IMAP account for the address holds a token under it.
    /// Google retires the oldest token when an account has too many, so
    /// one left unrevoked does no harm.
    async fn revoke_unless_shared(&self, token: &str, address: &EmailAddress) {
        self.grants
            .revoke_unless_shared(&self.oauth, token, address, Provider::Google)
            .await;
    }

    /// Run a secret-store call on a blocking thread.
    async fn secret<T: Send + 'static>(
        &self,
        account: &EmailAddress,
        call: impl FnOnce(&dyn SecretStore, &EmailAddress) -> KeychainResult<T> + Send + 'static,
    ) -> SyncResult<T> {
        let secrets = Arc::clone(&self.secrets);
        let account = account.clone();
        let result = tokio::task::spawn_blocking(move || call(secrets.as_ref(), &account))
            .await
            .map_err(|e| KeychainError::new(format!("keychain task failed: {e}")))?;
        Ok(result?)
    }
}
