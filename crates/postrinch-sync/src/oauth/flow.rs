//! One provider's OAuth requests: the authorization URL, the token
//! endpoint (code exchange and refresh), userinfo and revocation.

use std::time::Duration;

use postrinch_model::{EmailAddress, SignInReason};
use url::Url;

use super::tokens::{UserInfo, from_id_token};
use super::{Identified, Identity, OAuthSpec, TokenError, Tokens, missing_scopes};
use crate::{SyncError, SyncResult};

/// Tries at reaching the token endpoint, 2, 4 and 8 seconds apart.
const CONNECT_ATTEMPTS: u32 = 4;

/// A provider's flow: its [`OAuthSpec`] and an HTTP client.
#[derive(Clone)]
pub(crate) struct OAuthFlow {
    pub spec: OAuthSpec,
    pub http: reqwest::Client,
}

impl OAuthFlow {
    pub(crate) fn new(spec: OAuthSpec, http: reqwest::Client) -> Self {
        Self { spec, http }
    }

    /// The URL that starts sign-in in the browser. `login_hint` picks the
    /// account; without it the provider asks which.
    pub(crate) fn authorize_url(
        &self,
        redirect_uri: &str,
        challenge: &str,
        state: &str,
        login_hint: Option<&EmailAddress>,
    ) -> SyncResult<Url> {
        let mut url = Url::parse(&self.spec.endpoints.authorize)
            .map_err(|e| SyncError::Config(format!("auth_uri: {e}")))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("client_id", &self.spec.client_id)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("response_type", "code")
                .append_pair("scope", &self.spec.scope())
                .append_pair("code_challenge", challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("state", state);
            for (name, value) in (self.spec.authorize_params)(login_hint) {
                q.append_pair(name, &value);
            }
            if let Some(hint) = login_hint {
                q.append_pair("login_hint", &hint.to_string());
            }
        }
        Ok(url)
    }

    pub(crate) async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> SyncResult<Tokens> {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ])
        .await
    }

    /// A new access token, and with providers that rotate them a new
    /// refresh token. A refresh response that lists its scopes is checked
    /// like a sign-in; one that leaves them out is taken to keep the
    /// original grant's.
    pub(crate) async fn refresh(&self, refresh_token: &str) -> SyncResult<Tokens> {
        let tokens = self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .await?;
        if !tokens.scope.is_empty() {
            self.check_scopes(&tokens.scope)?;
        }
        Ok(tokens)
    }

    /// Refuse a grant that left out a scope the provider's spec requires.
    pub(crate) fn check_scopes(&self, granted: &str) -> SyncResult<()> {
        let missing = missing_scopes(self.spec.required_scopes, granted);
        if missing.is_empty() {
            Ok(())
        } else {
            Err(SyncError::NeedsSignIn(SignInReason::ScopesMissing {
                missing,
            }))
        }
    }

    async fn token_request(&self, params: &[(&str, &str)]) -> SyncResult<Tokens> {
        let scope = self.spec.scope();
        let mut form: Vec<(&str, &str)> = params.to_vec();
        form.push(("client_id", &self.spec.client_id));
        if let Some(secret) = &self.spec.client_secret {
            form.push(("client_secret", secret));
        }
        if self.spec.scope_in_token_requests {
            form.push(("scope", &scope));
        }
        // Retry only a request that could not connect: it never reached
        // the provider, so a one-time authorization code is still unspent.
        let mut attempt = 0;
        let response = loop {
            attempt += 1;
            match self
                .http
                .post(&self.spec.endpoints.token)
                .form(&form)
                .send()
                .await
            {
                Ok(response) => break response,
                Err(e) if e.is_connect() && attempt < CONNECT_ATTEMPTS => {
                    tracing::debug!("token endpoint unreachable, retrying: {e}");
                    tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                }
                Err(e) => return Err(e.into()),
            }
        };
        let status = response.status();
        let body = response.text().await?;
        if status.is_success() {
            return serde_json::from_str(&body).map_err(|e| {
                SyncError::api(self.spec.names.token, status.as_u16(), e.to_string())
            });
        }
        Err(self.token_error(status.as_u16(), &body))
    }

    /// What a failed token request means. A server error is always one to
    /// try again, whatever its body says; otherwise the provider's spec
    /// reads the error.
    pub(crate) fn token_error(&self, status: u16, body: &str) -> SyncError {
        match serde_json::from_str::<TokenError>(body) {
            Ok(e) if status < 500 => (self.spec.classify_error)(status, e),
            Ok(e) => SyncError::api(
                self.spec.names.token,
                status,
                format!("{}: {}", e.error, e.detail()),
            ),
            Err(_) => SyncError::api(self.spec.names.token, status, body.to_owned()),
        }
    }

    /// Who signed in, from the tokens a code exchange returned.
    pub(crate) async fn identify(&self, tokens: &Tokens) -> SyncResult<Identified> {
        match self.spec.identity {
            Identity::Userinfo => {
                Identified::from_userinfo(self.userinfo(&tokens.access_token).await?)
            }
            Identity::IdToken => {
                let id_token = tokens.id_token.as_deref().ok_or_else(|| {
                    SyncError::SignIn(format!("{} sent no ID token", self.spec.names.provider))
                })?;
                from_id_token(id_token)
            }
        }
    }

    pub(crate) async fn userinfo(&self, access_token: &str) -> SyncResult<UserInfo> {
        let endpoint = self.spec.endpoints.userinfo.as_deref().ok_or_else(|| {
            SyncError::Config(format!(
                "{} has no userinfo endpoint",
                self.spec.names.provider
            ))
        })?;
        let response = self
            .http
            .get(endpoint)
            .bearer_auth(access_token)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SyncError::api(
                self.spec.names.userinfo,
                status.as_u16(),
                body,
            ));
        }
        Ok(response.json().await?)
    }

    /// Revoke a refresh token Postrinch no longer uses. With a provider
    /// that has no revocation endpoint (Microsoft) there is nothing to
    /// do: the token lapses unused.
    pub(crate) async fn revoke(&self, token: &str) -> SyncResult<()> {
        let Some(endpoint) = self.spec.endpoints.revoke.as_deref() else {
            return Ok(());
        };
        let response = self
            .http
            .post(endpoint)
            .form(&[("token", token)])
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(SyncError::api(
            self.spec.names.revoke,
            status.as_u16(),
            body,
        ))
    }
}
