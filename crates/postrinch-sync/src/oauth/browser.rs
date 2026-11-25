//! The browser sign-in: from opening the provider's page to the tokens.

use std::time::Duration;

use postrinch_model::EmailAddress;

use super::OAuthFlow;
use super::Tokens;
use super::loopback::Loopback;
use super::pkce::Pkce;
use crate::{SyncError, SyncResult};

/// How long the person has to finish signing in in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// A grant from the browser: the tokens and the refresh token they came
/// with.
pub(crate) struct Granted {
    pub tokens: Tokens,
    pub refresh_token: String,
}

/// Run the browser sign-in with `flow`: listen for the redirect, hand
/// `open_browser` the provider's page, wait for the person to finish, and
/// exchange the code. Fails if the provider sends no refresh token.
pub(crate) async fn browser_sign_in(
    flow: &OAuthFlow,
    login_hint: Option<&EmailAddress>,
    open_browser: impl FnOnce(&str) -> std::io::Result<()>,
) -> SyncResult<Granted> {
    let pkce = Pkce::new();
    let loopback = Loopback::bind(flow.spec.redirect).await?;
    let redirect_uri = loopback.redirect_uri().to_owned();
    let url = flow.authorize_url(&redirect_uri, &pkce.challenge, &pkce.state, login_hint)?;
    open_browser(url.as_str())?;

    let code = loopback
        .wait_for_code(&pkce.state, flow.spec.names.provider, SIGN_IN_TIMEOUT)
        .await?;
    let tokens = flow
        .exchange_code(&code, &pkce.verifier, &redirect_uri)
        .await?;
    let refresh_token = tokens.refresh_token.clone().ok_or_else(|| {
        SyncError::SignIn(format!(
            "{} sent no refresh token",
            flow.spec.names.provider
        ))
    })?;
    Ok(Granted {
        tokens,
        refresh_token,
    })
}
