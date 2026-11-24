//! What one provider's OAuth flow is made of.

use postrinch_model::EmailAddress;
use serde::Deserialize;

use super::RedirectHost;
use crate::SyncError;

/// Where a provider's OAuth requests go.
#[derive(Debug, Clone)]
pub(crate) struct OAuthEndpoints {
    pub authorize: String,
    pub token: String,
    /// OpenID Connect userinfo, for [`Identity::Userinfo`].
    pub userinfo: Option<String>,
    /// RFC 7009 revocation. Microsoft has none for a refresh token.
    pub revoke: Option<String>,
}

/// How the address that signed in is learnt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Identity {
    /// The userinfo endpoint's `email` and `email_verified` (Google).
    Userinfo,
    /// The token response's `id_token`: its `email` claim, or failing that
    /// `preferred_username` (Microsoft, whose userinfo is Graph's and wants
    /// a token for Graph, not for Outlook).
    IdToken,
}

/// The services named in a provider's errors (`SyncError::Api`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Names {
    /// "Google", "Microsoft".
    pub provider: &'static str,
    pub token: &'static str,
    pub userinfo: &'static str,
    pub revoke: &'static str,
}

/// The authorization URL's provider-specific parameters, given the login
/// hint: `access_type` and `prompt` for Google, `prompt` for Microsoft.
pub(crate) type AuthorizeParams = fn(Option<&EmailAddress>) -> Vec<(&'static str, String)>;

/// A token endpoint's error answer (RFC 6749 §5.2), with Microsoft's
/// `error_codes` (the AADSTS numbers) where it sends them.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TokenError {
    pub error: String,
    #[serde(default)]
    pub error_description: String,
    #[serde(default)]
    pub error_codes: Vec<u64>,
}

impl TokenError {
    /// What to show: the first line of the description, or the error code
    /// when there is none.
    pub(crate) fn detail(&self) -> String {
        let line = super::first_line(&self.error_description);
        if line.is_empty() {
            self.error.clone()
        } else {
            line
        }
    }
}

/// One provider's OAuth client and what its answers mean.
#[derive(Clone)]
pub(crate) struct OAuthSpec {
    pub names: Names,
    pub endpoints: OAuthEndpoints,
    pub client_id: String,
    /// A desktop client's secret, for providers that issue one (Google).
    /// A public client (Microsoft) has none: PKCE alone protects the flow.
    pub client_secret: Option<String>,
    pub scopes: &'static [&'static str],
    /// The scopes a grant is refused without.
    pub required_scopes: &'static [&'static str],
    pub redirect: RedirectHost,
    pub identity: Identity,
    /// Whether code exchanges and refreshes name the scopes again, as
    /// Microsoft's v2 endpoint asks.
    pub scope_in_token_requests: bool,
    pub authorize_params: AuthorizeParams,
    /// What a token endpoint's error answer means: `NeedsSignIn` for a
    /// grant that no longer works, `Api` for a failure to try again.
    pub classify_error: fn(u16, TokenError) -> SyncError,
}

impl OAuthSpec {
    /// The scopes, as a request names them.
    pub(crate) fn scope(&self) -> String {
        self.scopes.join(" ")
    }
}
