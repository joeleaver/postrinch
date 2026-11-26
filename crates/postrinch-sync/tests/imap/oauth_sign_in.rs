//! Signing IMAP accounts in with OAuth in the browser, against mock
//! Microsoft and Google endpoints.

use postrinch_model::{EmailAddress, SignInMethod};
use postrinch_sync::SyncError;
use postrinch_sync::imap::{AccessTokens, ImapOAuth, ImapSecrets, OAuthClients, OAuthProvider};
use postrinch_sync_core::MemorySecrets;
use serde_json::json;
use std::sync::Arc;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::oauth_mock::{
    browser, kept_refresh_token, microsoft_grant, mock_token, no_browser, oauth, param,
};
use crate::support::{USER, account};

#[tokio::test]
async fn signs_in_with_microsoft() {
    let server = MockServer::start().await;
    // A public client: PKCE's verifier, the scopes again, and no secret.
    Mock::given(method("POST"))
        .and(path("/ms/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code_verifier="))
        .and(body_string_contains("client_id=ms-client"))
        .and(body_string_contains("IMAP.AccessAsUser.All"))
        .respond_with(ResponseTemplate::new(200).set_body_json(microsoft_grant("Ada@Example.org")))
        .expect(1)
        .mount(&server)
        .await;
    let (oauth, secrets) = oauth(&server);

    let signed_in = oauth
        .sign_in(
            OAuthProvider::Microsoft,
            &account(),
            browser(|page| {
                assert_eq!(page.path(), "/ms/authorize");
                assert_eq!(param(page, "client_id").unwrap(), "ms-client");
                assert_eq!(param(page, "login_hint").unwrap(), USER);
                let redirect = param(page, "redirect_uri").unwrap();
                assert!(redirect.starts_with("http://localhost:"), "{redirect}");
                let scope = param(page, "scope").unwrap();
                for s in ["IMAP.AccessAsUser.All", "SMTP.Send", "offline_access"] {
                    assert!(scope.contains(s), "{scope}");
                }
            }),
        )
        .await
        .unwrap();

    assert_eq!(signed_in.address, account());
    assert_eq!(signed_in.method, OAuthProvider::Microsoft.sign_in_method());
    assert_eq!(signed_in.display_name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(kept_refresh_token(&secrets).await, "ms-refresh");
    // The sign-in's access token is kept: no refresh is asked for.
    assert_eq!(oauth.access_token(&account()).await.unwrap(), "ms-access");
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| !String::from_utf8_lossy(&r.body).contains("client_secret"))
    );
}

#[tokio::test]
async fn signs_in_to_google_over_imap_for_mail_alone() {
    let server = MockServer::start().await;
    mock_token(
        &server,
        "/g/token",
        "authorization_code",
        200,
        json!({"access_token": "g-access", "expires_in": 3599, "refresh_token": "g-refresh",
               "scope": "openid https://www.googleapis.com/auth/userinfo.email https://mail.google.com/"}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/g/userinfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "email": USER, "email_verified": true, "name": "Ada"
        })))
        .mount(&server)
        .await;
    let (oauth, secrets) = oauth(&server);

    let signed_in = oauth
        .sign_in(
            OAuthProvider::Google,
            &account(),
            browser(|page| {
                assert_eq!(page.path(), "/g/auth");
                assert_eq!(
                    param(page, "scope").unwrap(),
                    "https://mail.google.com/ openid email"
                );
                assert_eq!(param(page, "access_type").unwrap(), "offline");
                let redirect = param(page, "redirect_uri").unwrap();
                assert!(redirect.starts_with("http://127.0.0.1:"), "{redirect}");
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        signed_in.method,
        SignInMethod::OAuth2 {
            provider: "google".into()
        }
    );
    let kept = secrets.get(&account()).await.unwrap().to_json();
    assert_eq!(
        kept,
        r#"{"kind":"oauth2","provider":"google","refresh_token":"g-refresh"}"#
    );
    assert_eq!(oauth.access_token(&account()).await.unwrap(), "g-access");
}

#[tokio::test]
async fn another_account_is_refused_and_nothing_kept() {
    let server = MockServer::start().await;
    mock_token(
        &server,
        "/ms/token",
        "authorization_code",
        200,
        microsoft_grant("bob@example.org"),
    )
    .await;
    let (oauth, secrets) = oauth(&server);

    let err = oauth
        .sign_in(OAuthProvider::Microsoft, &account(), browser(|_| {}))
        .await
        .unwrap_err();
    match err {
        SyncError::OtherAccount { asked, signed_in } => {
            assert_eq!(asked, account());
            assert_eq!(signed_in.key(), "bob@example.org");
        }
        other => panic!("expected another account, got {other:?}"),
    }
    assert!(secrets.get(&account()).await.is_err());
    let bob = EmailAddress::parse("bob@example.org").unwrap();
    assert!(secrets.get(&bob).await.is_err());
}

#[tokio::test]
async fn signing_in_again_replaces_the_grant() {
    let server = MockServer::start().await;
    mock_token(
        &server,
        "/ms/token",
        "authorization_code",
        200,
        microsoft_grant(USER),
    )
    .await;
    let (oauth, secrets) = oauth(&server);
    crate::oauth_mock::keep_grant(&secrets, "microsoft", "refused-long-ago").await;

    let method = OAuthProvider::Microsoft.sign_in_method();
    let again = oauth
        .sign_in_again(&account(), &method, browser(|_| {}))
        .await
        .unwrap();
    assert_eq!(again.method, method);
    assert_eq!(kept_refresh_token(&secrets).await, "ms-refresh");

    let password = oauth
        .sign_in_again(&account(), &SignInMethod::Password, no_browser)
        .await;
    assert!(matches!(password, Err(SyncError::SignIn(_))));
}

#[tokio::test]
async fn microsoft_without_its_client_file_is_not_set_up() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("google-oauth-client.json"),
        r#"{"installed":{"client_id":"g","client_secret":"s",
            "auth_uri":"https://accounts.google.com/o/oauth2/auth",
            "token_uri":"https://oauth2.googleapis.com/token"}}"#,
    )
    .unwrap();
    let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
    let google = Arc::new(MemorySecrets::default());
    let oauth =
        ImapOAuth::new(OAuthClients::load_from(dir.path()), secrets.clone(), google).unwrap();
    assert!(!oauth.is_set_up(OAuthProvider::Microsoft));
    assert!(oauth.is_set_up(OAuthProvider::Google));

    let err = oauth
        .sign_in(OAuthProvider::Microsoft, &account(), no_browser)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, SyncError::OAuthNotSetUp { provider, detail: None } if provider == "Microsoft"),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "Microsoft sign-in isn't set up in this copy of Postrinch"
    );

    // An account that already has a Microsoft grant says the same.
    crate::oauth_mock::keep_grant(&secrets, "microsoft", "rt").await;
    let err = oauth.access_token(&account()).await.unwrap_err();
    assert!(matches!(err, SyncError::OAuthNotSetUp { .. }), "{err:?}");
}

#[tokio::test]
async fn signing_out_forgets_the_grant() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/g/revoke"))
        .and(body_string_contains("token=g-refresh"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let (oauth, secrets) = oauth(&server);
    crate::oauth_mock::keep_grant(&secrets, "google", "g-refresh").await;
    oauth.sign_out(&account()).await.unwrap();
    assert!(secrets.get(&account()).await.is_err());

    // Microsoft takes nothing back; the secret goes all the same.
    crate::oauth_mock::keep_grant(&secrets, "microsoft", "ms-refresh").await;
    oauth.sign_out(&account()).await.unwrap();
    assert!(secrets.get(&account()).await.is_err());
}
