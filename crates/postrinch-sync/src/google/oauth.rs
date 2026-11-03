//! Google's OAuth endpoints: the authorization URL, the token endpoint
//! (code exchange and refresh), userinfo and revocation.

use postrinch_model::{EmailAddress, SignInReason};
use std::time::Duration;

use serde::Deserialize;
use url::Url;

use super::ClientConfig;
use crate::{SyncError, SyncResult};

/// Tries at reaching the token endpoint, 2, 4 and 8 seconds apart.
const CONNECT_ATTEMPTS: u32 = 4;

/// Every scope Postrinch asks for, together, at sign-in.
pub const SCOPES: &[&str] = &[
    "openid",
    "email",
    "https://mail.google.com/",
    "https://www.googleapis.com/auth/calendar",
];

/// The scopes Postrinch cannot work without. Google lets a person untick
/// scopes on the consent screen; a grant without these is refused.
const REQUIRED_SCOPES: &[&str] = &[
    "https://mail.google.com/",
    "https://www.googleapis.com/auth/calendar",
];

/// Where the OAuth requests go. Production values come from the client
/// file and Google's documentation; tests point them at a mock server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub auth: String,
    pub token: String,
    pub userinfo: String,
    pub revoke: String,
}

impl Endpoints {
    pub fn google(config: &ClientConfig) -> Self {
        Self {
            auth: config.auth_uri.clone(),
            token: config.token_uri.clone(),
            userinfo: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
        }
    }
}

/// A successful token response.
#[derive(Debug, Deserialize)]
pub(crate) struct Tokens {
    pub access_token: String,
    pub expires_in: u64,
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: String,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UserInfo {
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
    pub name: Option<String>,
}

pub(crate) struct OAuth {
    pub config: ClientConfig,
    pub endpoints: Endpoints,
    pub http: reqwest::Client,
}

impl OAuth {
    /// The URL that starts sign-in in the browser. `login_hint` picks the
    /// account when one is signing in again; without it Google asks which.
    pub(crate) fn authorize_url(
        &self,
        redirect_uri: &str,
        challenge: &str,
        state: &str,
        login_hint: Option<&EmailAddress>,
    ) -> SyncResult<Url> {
        let mut url = Url::parse(&self.endpoints.auth)
            .map_err(|e| SyncError::Config(format!("auth_uri: {e}")))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("client_id", &self.config.client_id)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("response_type", "code")
                .append_pair("scope", &SCOPES.join(" "))
                .append_pair("code_challenge", challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("state", state)
                // offline: a refresh token. consent: Google only sends a
                // refresh token with a consent, so ask for it every time.
                .append_pair("access_type", "offline")
                .append_pair("include_granted_scopes", "true");
            match login_hint {
                Some(hint) => q
                    .append_pair("login_hint", &hint.to_string())
                    .append_pair("prompt", "consent"),
                None => q.append_pair("prompt", "consent select_account"),
            };
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

    /// A new access token. A refresh response that lists its scopes is
    /// checked like a sign-in; one that leaves them out is taken to keep
    /// the original grant's.
    pub(crate) async fn refresh(&self, refresh_token: &str) -> SyncResult<Tokens> {
        let tokens = self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .await?;
        if !tokens.scope.is_empty() {
            check_scopes(&tokens.scope)?;
        }
        Ok(tokens)
    }

    async fn token_request(&self, params: &[(&str, &str)]) -> SyncResult<Tokens> {
        let mut form: Vec<(&str, &str)> = params.to_vec();
        form.push(("client_id", &self.config.client_id));
        form.push(("client_secret", &self.config.client_secret));
        // Retry only a request that could not connect: it never reached
        // Google, so a one-time authorization code is still unspent.
        let mut attempt = 0;
        let response = loop {
            attempt += 1;
            match self
                .http
                .post(&self.endpoints.token)
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
            return serde_json::from_str(&body)
                .map_err(|e| SyncError::api("Google OAuth", status.as_u16(), e.to_string()));
        }
        Err(token_error(status.as_u16(), &body))
    }

    pub(crate) async fn userinfo(&self, access_token: &str) -> SyncResult<UserInfo> {
        let response = self
            .http
            .get(&self.endpoints.userinfo)
            .bearer_auth(access_token)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SyncError::api("Google userinfo", status.as_u16(), body));
        }
        Ok(response.json().await?)
    }

    /// Revoke a refresh token Postrinch no longer uses, so it does not count
    /// against Google's 100 tokens per account and client.
    pub(crate) async fn revoke(&self, token: &str) -> SyncResult<()> {
        let response = self
            .http
            .post(&self.endpoints.revoke)
            .form(&[("token", token)])
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(SyncError::api("Google revoke", status.as_u16(), body))
    }
}

/// Refuse a grant that left out a scope Postrinch needs.
pub(crate) fn check_scopes(granted: &str) -> SyncResult<()> {
    let granted: Vec<&str> = granted.split_whitespace().collect();
    let missing: Vec<String> = REQUIRED_SCOPES
        .iter()
        .filter(|s| !granted.contains(s))
        .map(|s| s.to_string())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(SyncError::NeedsSignIn(SignInReason::ScopesMissing {
            missing,
        }))
    }
}

/// What a failed token request means. `invalid_grant` is every way a
/// refresh token stops working; Google does not say which.
fn token_error(status: u16, body: &str) -> SyncError {
    let Ok(e) = serde_json::from_str::<TokenError>(body) else {
        return SyncError::api("Google OAuth", status, body.to_owned());
    };
    let detail = if e.error_description.is_empty() {
        e.error.clone()
    } else {
        e.error_description
    };
    match e.error.as_str() {
        "invalid_grant" => SyncError::NeedsSignIn(SignInReason::CredentialRejected { detail }),
        "admin_policy_enforced" => SyncError::NeedsSignIn(SignInReason::AdminPolicy { detail }),
        _ => SyncError::api("Google OAuth", status, format!("{}: {detail}", e.error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_scopes_need_sign_in() {
        check_scopes("openid https://mail.google.com/ https://www.googleapis.com/auth/calendar")
            .unwrap();
        let err = check_scopes("openid https://mail.google.com/").unwrap_err();
        assert!(matches!(
            err,
            SyncError::NeedsSignIn(SignInReason::ScopesMissing { missing }) if missing == ["https://www.googleapis.com/auth/calendar"]
        ));
    }

    #[test]
    fn classifies_token_errors() {
        let revoked = token_error(
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        );
        assert!(matches!(
            revoked,
            SyncError::NeedsSignIn(SignInReason::CredentialRejected { .. })
        ));
        let policy = token_error(400, r#"{"error":"admin_policy_enforced"}"#);
        assert!(matches!(
            policy,
            SyncError::NeedsSignIn(SignInReason::AdminPolicy { .. })
        ));
        assert!(matches!(
            token_error(500, "oops"),
            SyncError::Api { status: 500, .. }
        ));
    }
}
