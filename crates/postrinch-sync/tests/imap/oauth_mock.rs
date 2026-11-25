//! Mock OAuth providers for IMAP accounts: Microsoft's and Google's
//! endpoints on a wiremock server, a stand-in browser that follows the
//! redirect as the provider would, and the clients pointing at them.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use postrinch_sync::MemorySecrets;
use postrinch_sync::google::{ClientConfig, Endpoints, GoogleAuth};
use postrinch_sync::imap::{ImapOAuth, ImapSecret, ImapSecrets, MicrosoftClient, OAuthClients};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::support::{USER, account};

pub const MICROSOFT_SCOPES: &str =
    "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send";

/// Google's client at `server`, and its endpoints there: the one both
/// Google accounts and Google over IMAP sign in with.
fn google_client(server: &MockServer) -> (ClientConfig, Endpoints) {
    let google = ClientConfig {
        client_id: "g-client".into(),
        client_secret: "g-secret".into(),
        auth_uri: format!("{}/g/auth", server.uri()),
        token_uri: format!("{}/g/token", server.uri()),
    };
    let endpoints = Endpoints {
        auth: google.auth_uri.clone(),
        token: google.token_uri.clone(),
        userinfo: format!("{}/g/userinfo", server.uri()),
        revoke: format!("{}/g/revoke", server.uri()),
    };
    (google, endpoints)
}

/// Microsoft and Google over IMAP, both at `server`.
pub fn clients(server: &MockServer) -> OAuthClients {
    let microsoft = MicrosoftClient::new("ms-client").with_endpoints(
        format!("{}/ms/authorize", server.uri()),
        format!("{}/ms/token", server.uri()),
    );
    let (google, endpoints) = google_client(server);
    OAuthClients::none()
        .with_microsoft(&microsoft)
        .with_google_endpoints(&google, endpoints)
}

/// The keychain's two services, in memory: `postrinch.imap`'s and
/// `postrinch.google`'s.
#[derive(Clone)]
pub struct Keychains {
    pub imap: ImapSecrets,
    pub google: Arc<MemorySecrets>,
}

impl Keychains {
    pub fn new() -> Self {
        Self {
            imap: ImapSecrets::new(Arc::new(MemorySecrets::default())),
            google: Arc::new(MemorySecrets::default()),
        }
    }

    /// An [`ImapOAuth`] over `server`'s providers.
    pub fn imap_oauth(&self, server: &MockServer) -> Arc<ImapOAuth> {
        Arc::new(ImapOAuth::new(clients(server), self.imap.clone(), self.google.clone()).unwrap())
    }

    /// A Google account's sign-in, with Google's client at `server`.
    pub fn google_auth(&self, server: &MockServer) -> GoogleAuth {
        let (config, endpoints) = google_client(server);
        GoogleAuth::with_endpoints(config, endpoints, self.google.clone(), self.imap.clone())
            .unwrap()
    }
}

/// An [`ImapOAuth`] over `server`'s providers, and the secrets it keeps.
pub fn oauth(server: &MockServer) -> (Arc<ImapOAuth>, ImapSecrets) {
    let keychains = Keychains::new();
    (keychains.imap_oauth(server), keychains.imap)
}

/// The account's secret, an OAuth grant with `provider`.
pub async fn keep_grant(secrets: &ImapSecrets, provider: &str, refresh_token: &str) {
    let secret = ImapSecret::OAuth2 {
        provider: provider.into(),
        refresh_token: refresh_token.into(),
    };
    secrets.set(&account(), &secret).await.unwrap();
}

pub async fn kept_refresh_token(secrets: &ImapSecrets) -> String {
    match secrets.get(&account()).await.unwrap() {
        ImapSecret::OAuth2 { refresh_token, .. } => refresh_token,
        other => panic!("expected an OAuth grant, got {other:?}"),
    }
}

/// An unsigned ID token with `claims`, as a token endpoint returns one.
pub fn id_token(claims: Value) -> String {
    let part = |v: &Value| URL_SAFE_NO_PAD.encode(v.to_string());
    format!(
        "{}.{}.signature",
        part(&json!({"alg": "RS256"})),
        part(&claims)
    )
}

/// A parameter of a URL's query.
pub fn param(url: &url::Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

/// Plays the browser: checks the authorization URL with `check`, then
/// calls the loopback redirect with a code and the same state, as the
/// provider does after consent.
pub fn browser(check: impl FnOnce(&url::Url)) -> impl FnOnce(&str) -> std::io::Result<()> {
    browser_via(None, check)
}

/// [`browser`], reaching the redirect's host at `address` (`[::1]`,
/// `127.0.0.1`) rather than wherever its name resolves first.
pub fn browser_via(
    address: Option<&'static str>,
    check: impl FnOnce(&url::Url),
) -> impl FnOnce(&str) -> std::io::Result<()> {
    move |page: &str| {
        let page = url::Url::parse(page).unwrap();
        check(&page);
        assert_eq!(param(&page, "code_challenge_method").unwrap(), "S256");
        let mut redirect = url::Url::parse(&param(&page, "redirect_uri").unwrap()).unwrap();
        if let Some(address) = address {
            redirect.set_host(Some(address)).unwrap();
        }
        redirect
            .query_pairs_mut()
            .append_pair("state", &param(&page, "state").unwrap())
            .append_pair("code", "the-code");
        tokio::spawn(async move {
            reqwest::get(redirect).await.unwrap();
        });
        Ok(())
    }
}

/// A browser that must never be opened.
pub fn no_browser(_: &str) -> std::io::Result<()> {
    panic!("the browser was opened")
}

/// Answers `POST <path>` whose body has `grant_type=<grant>` with `body`.
pub async fn mock_token(
    server: &MockServer,
    token_path: &str,
    grant: &str,
    status: u16,
    body: Value,
) {
    Mock::given(method("POST"))
        .and(path(token_path))
        .and(body_string_contains(format!("grant_type={grant}")))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

/// Google's answers to a sign-in as `USER` with `scope`: its refresh
/// token is `g-new`, its access token `g-access`.
pub async fn mock_google_sign_in(server: &MockServer, scope: &str) {
    mock_token(
        server,
        "/g/token",
        "authorization_code",
        200,
        json!({"access_token": "g-access", "expires_in": 3599,
               "refresh_token": "g-new", "scope": scope}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/g/userinfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "email": USER, "email_verified": true, "name": "Ada"
        })))
        .mount(server)
        .await;
}

/// Expects `times` revocations of `token` at Google.
pub async fn expect_revoke(server: &MockServer, token: &str, times: u64) {
    Mock::given(method("POST"))
        .and(path("/g/revoke"))
        .and(body_string_contains(format!("token={token}")))
        .respond_with(ResponseTemplate::new(200))
        .expect(times)
        .mount(server)
        .await;
}

/// Microsoft's answer to a code exchange for `email`.
pub fn microsoft_grant(email: &str) -> Value {
    json!({
        "token_type": "Bearer",
        "scope": MICROSOFT_SCOPES,
        "expires_in": 3599,
        "access_token": "ms-access",
        "refresh_token": "ms-refresh",
        "id_token": id_token(json!({"email": email, "name": "Ada Lovelace"})),
    })
}
