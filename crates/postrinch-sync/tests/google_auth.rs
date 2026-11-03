//! Sign-in and token refresh against a mock of Google's endpoints, with a
//! stand-in browser that follows the redirect the way Google would.

use std::sync::Arc;

use postrinch_model::{EmailAddress, SignInReason};
use postrinch_sync::SyncError;
use postrinch_sync::google::{ClientConfig, Endpoints, GoogleAuth, MemorySecrets, SecretStore};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ALL_SCOPES: &str = "openid https://www.googleapis.com/auth/userinfo.email \
    https://mail.google.com/ https://www.googleapis.com/auth/calendar";

fn auth(server: &MockServer, secrets: Arc<MemorySecrets>) -> GoogleAuth {
    let config = ClientConfig {
        client_id: "client".into(),
        client_secret: "secret".into(),
        auth_uri: format!("{}/auth", server.uri()),
        token_uri: format!("{}/token", server.uri()),
    };
    let endpoints = Endpoints {
        auth: config.auth_uri.clone(),
        token: config.token_uri.clone(),
        userinfo: format!("{}/userinfo", server.uri()),
        revoke: format!("{}/revoke", server.uri()),
    };
    GoogleAuth::with_endpoints(config, endpoints, secrets)
}

/// Plays the browser: reads the authorization URL and calls the loopback
/// redirect with a code and the same state, as Google does after consent.
fn browser(url: &str) -> std::io::Result<()> {
    let url = url::Url::parse(url).unwrap();
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .unwrap()
            .1
            .into_owned()
    };
    assert_eq!(param("code_challenge_method"), "S256");
    assert_eq!(param("access_type"), "offline");
    let redirect = format!(
        "{}?state={}&code=the-code",
        param("redirect_uri"),
        param("state")
    );
    tokio::spawn(async move {
        reqwest::get(redirect).await.unwrap();
    });
    Ok(())
}

async fn mock_tokens(server: &MockServer, grant: &str, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!("grant_type={grant}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mock_userinfo(server: &MockServer, email: &str) {
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "email": email, "email_verified": true, "name": "Ada Lovelace"
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn sign_in_stores_the_refresh_token_and_revokes_the_old_one() {
    let server = MockServer::start().await;
    mock_tokens(
        &server,
        "authorization_code",
        serde_json::json!({"access_token": "at-1", "expires_in": 3599, "refresh_token": "rt-new", "scope": ALL_SCOPES}),
    )
    .await;
    mock_userinfo(&server, "Ada@Example.org").await;
    Mock::given(method("POST"))
        .and(path("/revoke"))
        .and(body_string_contains("token=rt-old"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let secrets = Arc::new(MemorySecrets::default());
    let ada = EmailAddress::parse("ada@example.org").unwrap();
    secrets.set(&ada, "rt-old").unwrap();
    let auth = auth(&server, secrets.clone());

    let account = auth.sign_in(None, browser).await.unwrap();
    assert_eq!(account.address.key(), "ada@example.org");
    assert_eq!(account.display_name.as_deref(), Some("Ada Lovelace"));
    assert!(account.is_active());
    assert_eq!(secrets.get(&ada).unwrap().as_deref(), Some("rt-new"));
    // The access token from sign-in is cached: no refresh request needed.
    assert_eq!(auth.access_token(&ada).await.unwrap(), "at-1");
}

#[tokio::test]
async fn sign_in_without_mail_scope_is_refused() {
    let server = MockServer::start().await;
    mock_tokens(
        &server,
        "authorization_code",
        serde_json::json!({"access_token": "at", "expires_in": 3599, "refresh_token": "rt", "scope": "openid email"}),
    )
    .await;
    let auth = auth(&server, Arc::new(MemorySecrets::default()));
    let err = auth.sign_in(None, browser).await.unwrap_err();
    assert!(
        matches!(
            err,
            SyncError::NeedsSignIn(SignInReason::ScopesMissing { .. })
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn signing_in_as_someone_else_is_refused() {
    let server = MockServer::start().await;
    mock_tokens(
        &server,
        "authorization_code",
        serde_json::json!({"access_token": "at", "expires_in": 3599, "refresh_token": "rt", "scope": ALL_SCOPES}),
    )
    .await;
    mock_userinfo(&server, "bob@example.org").await;
    Mock::given(method("POST"))
        .and(path("/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let secrets = Arc::new(MemorySecrets::default());
    let auth = auth(&server, secrets.clone());
    let ada = EmailAddress::parse("ada@example.org").unwrap();
    let err = auth.sign_in(Some(&ada), browser).await.unwrap_err();
    assert!(
        matches!(err, SyncError::SignIn(ref m) if m.contains("bob@example.org")),
        "{err:?}"
    );
    let bob = EmailAddress::parse("bob@example.org").unwrap();
    assert_eq!(secrets.get(&bob).unwrap(), None);
}

#[tokio::test]
async fn refreshes_and_reports_a_revoked_token() {
    let server = MockServer::start().await;
    let secrets = Arc::new(MemorySecrets::default());
    let ada = EmailAddress::parse("ada@example.org").unwrap();
    let auth = auth(&server, secrets.clone());

    let err = auth.access_token(&ada).await.unwrap_err();
    assert!(matches!(
        err,
        SyncError::NeedsSignIn(SignInReason::NoCredential)
    ));

    secrets.set(&ada, "rt").unwrap();
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"access_token": "fresh", "expires_in": 3599, "scope": ALL_SCOPES}),
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert_eq!(auth.access_token(&ada).await.unwrap(), "fresh");
    assert_eq!(auth.access_token(&ada).await.unwrap(), "fresh");

    auth.invalidate(&ada).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            serde_json::json!({"error": "invalid_grant", "error_description": "Token has been expired or revoked."}),
        ))
        .mount(&server)
        .await;
    let err = auth.access_token(&ada).await.unwrap_err();
    assert!(
        matches!(
            err,
            SyncError::NeedsSignIn(SignInReason::CredentialRejected { .. })
        ),
        "{err:?}"
    );
}
