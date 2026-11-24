//! Google's OAuth: its endpoints, the scopes Postrinch asks for, and what
//! its token errors mean. The flow itself is `crate::oauth`'s. Two specs
//! share the client: Google accounts (the Gmail and Calendar APIs) and
//! Google over IMAP, whose grant is for mail alone.

use postrinch_model::{EmailAddress, SignInReason};

use super::ClientConfig;
use crate::oauth::{
    AuthorizeParams, Identity, Names, OAuthEndpoints, OAuthSpec, RedirectHost, TokenError,
};
use crate::{SyncError, SyncResult};

/// Every scope Postrinch asks for, together, at sign-in.
pub const SCOPES: &[&str] = &[
    "openid",
    "email",
    "https://mail.google.com/",
    "https://www.googleapis.com/auth/calendar",
    SETTINGS_SCOPE,
];

/// Making Gmail filters for rules. Not required: without it, rules run in
/// Postrinch (docs/rules.md, 7).
pub const SETTINGS_SCOPE: &str = "https://www.googleapis.com/auth/gmail.settings.basic";

/// The scopes Postrinch cannot work without. Google lets a person untick
/// scopes on the consent screen; a grant without these is refused.
const REQUIRED_SCOPES: &[&str] = &[
    "https://mail.google.com/",
    "https://www.googleapis.com/auth/calendar",
];

/// Google over IMAP: `https://mail.google.com/` covers IMAP and SMTP.
const IMAP_SCOPES: &[&str] = &["https://mail.google.com/", "openid", "email"];
const IMAP_REQUIRED_SCOPES: &[&str] = &["https://mail.google.com/"];

const NAMES: Names = Names {
    provider: "Google",
    token: "Google OAuth",
    userinfo: "Google userinfo",
    revoke: "Google revoke",
};

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

impl From<Endpoints> for OAuthEndpoints {
    fn from(e: Endpoints) -> Self {
        Self {
            authorize: e.auth,
            token: e.token,
            userinfo: Some(e.userinfo),
            revoke: Some(e.revoke),
        }
    }
}

/// Google accounts: mail, calendars and filters over Google's APIs.
pub(crate) fn api_spec(config: &ClientConfig, endpoints: Endpoints) -> OAuthSpec {
    spec(
        config,
        endpoints,
        SCOPES,
        REQUIRED_SCOPES,
        api_authorize_params,
    )
}

/// Google over IMAP and SMTP. Its grant asks for mail only, and does not
/// take in the scopes a Google account with the address has granted.
pub(crate) fn imap_spec(config: &ClientConfig, endpoints: Endpoints) -> OAuthSpec {
    spec(
        config,
        endpoints,
        IMAP_SCOPES,
        IMAP_REQUIRED_SCOPES,
        imap_authorize_params,
    )
}

fn spec(
    config: &ClientConfig,
    endpoints: Endpoints,
    scopes: &'static [&'static str],
    required_scopes: &'static [&'static str],
    authorize_params: AuthorizeParams,
) -> OAuthSpec {
    OAuthSpec {
        names: NAMES,
        endpoints: endpoints.into(),
        client_id: config.client_id.clone(),
        client_secret: Some(config.client_secret.clone()),
        scopes,
        required_scopes,
        redirect: RedirectHost::Ip,
        identity: Identity::Userinfo,
        scope_in_token_requests: false,
        authorize_params,
        classify_error: classify,
    }
}

/// offline: a refresh token. consent: Google only sends a refresh token
/// with a consent, so ask for it every time.
fn api_authorize_params(login_hint: Option<&EmailAddress>) -> Vec<(&'static str, String)> {
    let mut params = vec![
        ("access_type", "offline".to_owned()),
        ("include_granted_scopes", "true".to_owned()),
    ];
    params.push(prompt(login_hint));
    params
}

fn imap_authorize_params(login_hint: Option<&EmailAddress>) -> Vec<(&'static str, String)> {
    vec![("access_type", "offline".to_owned()), prompt(login_hint)]
}

fn prompt(login_hint: Option<&EmailAddress>) -> (&'static str, String) {
    match login_hint {
        Some(_) => ("prompt", "consent".to_owned()),
        None => ("prompt", "consent select_account".to_owned()),
    }
}

/// Refuse a Google account's grant that left out a scope Postrinch needs.
pub(crate) fn check_scopes(granted: &str) -> SyncResult<()> {
    let missing = crate::oauth::missing_scopes(REQUIRED_SCOPES, granted);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(SyncError::NeedsSignIn(SignInReason::ScopesMissing {
            missing,
        }))
    }
}

/// What a token error means. `invalid_grant` is every way a refresh token
/// stops working; Google does not say which.
fn classify(status: u16, e: TokenError) -> SyncError {
    let detail = e.detail();
    match e.error.as_str() {
        "invalid_grant" => SyncError::NeedsSignIn(SignInReason::CredentialRejected { detail }),
        "admin_policy_enforced" => SyncError::NeedsSignIn(SignInReason::AdminPolicy { detail }),
        _ => SyncError::api(NAMES.token, status, format!("{}: {detail}", e.error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::OAuthFlow;

    fn flow() -> OAuthFlow {
        let config = ClientConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_uri: "https://accounts.google.com/o/oauth2/auth".into(),
            token_uri: "https://oauth2.googleapis.com/token".into(),
        };
        let endpoints = Endpoints::google(&config);
        OAuthFlow::new(api_spec(&config, endpoints), reqwest::Client::new())
    }

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
        let flow = flow();
        let revoked = flow.token_error(
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        );
        assert!(matches!(
            revoked,
            SyncError::NeedsSignIn(SignInReason::CredentialRejected { .. })
        ));
        let policy = flow.token_error(400, r#"{"error":"admin_policy_enforced"}"#);
        assert!(matches!(
            policy,
            SyncError::NeedsSignIn(SignInReason::AdminPolicy { .. })
        ));
        assert!(matches!(
            flow.token_error(500, "oops"),
            SyncError::Api { status: 500, .. }
        ));
    }

    #[test]
    fn the_imap_grant_is_for_mail_alone() {
        let config = ClientConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_uri: "https://accounts.google.com/o/oauth2/auth".into(),
            token_uri: "https://oauth2.googleapis.com/token".into(),
        };
        let spec = imap_spec(&config, Endpoints::google(&config));
        assert_eq!(spec.scope(), "https://mail.google.com/ openid email");
        let params = (spec.authorize_params)(None);
        assert!(!params.iter().any(|(k, _)| *k == "include_granted_scopes"));
    }
}
