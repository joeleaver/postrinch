//! IMAP accounts' access tokens from their OAuth grants: refreshing,
//! keeping, sharing one refresh, rotated refresh tokens, refusals, and a
//! real XOAUTH2 sign-in with the token.

use std::time::Duration;

use postrinch_mailtest::{ImapConfig, ImapServer};
use postrinch_model::{Security, SignInReason};
use postrinch_sync::SyncError;
use postrinch_sync::imap::{AccessTokens, ImapSecret, ImapSession};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::oauth_mock::{MICROSOFT_SCOPES, keep_grant, kept_refresh_token, mock_token, oauth};
use crate::support::{USER, account, connector, microsoft, server, settings};

fn refreshed(access: &str, expires_in: u64) -> serde_json::Value {
    json!({"access_token": access, "expires_in": expires_in, "scope": MICROSOFT_SCOPES})
}

#[tokio::test]
async fn concurrent_callers_share_one_refresh_and_the_token_is_kept() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ms/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=rt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(refreshed("at-1", 3599))
                .set_delay(Duration::from_millis(200)),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    let (oauth, secrets) = oauth(&server);
    keep_grant(&secrets, "microsoft", "rt").await;

    let callers: Vec<_> = (0..5)
        .map(|_| {
            let oauth = oauth.clone();
            tokio::spawn(async move { oauth.access_token(&account()).await })
        })
        .collect();
    for caller in callers {
        assert_eq!(caller.await.unwrap().unwrap(), "at-1");
    }
    // Kept: asked again, no refresh.
    assert_eq!(oauth.access_token(&account()).await.unwrap(), "at-1");

    // Refused by a server: forgotten, and the next call refreshes.
    oauth.invalidate(&account()).await;
    mock_token(
        &server,
        "/ms/token",
        "refresh_token",
        200,
        refreshed("at-2", 3599),
    )
    .await;
    assert_eq!(oauth.access_token(&account()).await.unwrap(), "at-2");
}

#[tokio::test]
async fn a_token_about_to_expire_is_refreshed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ms/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(refreshed("short", 30)))
        .expect(2)
        .mount(&server)
        .await;
    let (oauth, secrets) = oauth(&server);
    keep_grant(&secrets, "microsoft", "rt").await;
    // Thirty seconds is inside the margin: each call refreshes.
    oauth.access_token(&account()).await.unwrap();
    oauth.access_token(&account()).await.unwrap();
}

#[tokio::test]
async fn a_rotated_refresh_token_is_kept_and_used() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/ms/token"))
        .and(body_string_contains("refresh_token=rt-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "at-1", "expires_in": 3599, "refresh_token": "rt-2",
            "scope": MICROSOFT_SCOPES
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/ms/token"))
        .and(body_string_contains("refresh_token=rt-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(refreshed("at-2", 3599)))
        .expect(1)
        .mount(&server)
        .await;
    let (oauth, secrets) = oauth(&server);
    keep_grant(&secrets, "microsoft", "rt-1").await;

    assert_eq!(oauth.access_token(&account()).await.unwrap(), "at-1");
    assert_eq!(kept_refresh_token(&secrets).await, "rt-2");
    oauth.invalidate(&account()).await;
    assert_eq!(oauth.access_token(&account()).await.unwrap(), "at-2");
    // A refresh that sends no new one leaves the kept one alone.
    assert_eq!(kept_refresh_token(&secrets).await, "rt-2");
}

/// What the account's token comes to when Microsoft answers a refresh
/// with `status` and `body`.
async fn refresh_refused(status: u16, body: serde_json::Value) -> SyncError {
    let server = MockServer::start().await;
    mock_token(&server, "/ms/token", "refresh_token", status, body).await;
    let (oauth, secrets) = oauth(&server);
    keep_grant(&secrets, "microsoft", "rt").await;
    oauth.access_token(&account()).await.unwrap_err()
}

#[tokio::test]
async fn an_expired_refresh_token_needs_sign_in() {
    let err = refresh_refused(
        400,
        json!({"error": "invalid_grant",
               "error_description": "AADSTS700082: The refresh token has expired due to inactivity.\r\nTrace ID: 1\r\nCorrelation ID: 2",
               "error_codes": [700082]}),
    )
    .await;
    match err {
        SyncError::NeedsSignIn(SignInReason::CredentialRejected { detail }) => assert_eq!(
            detail,
            "AADSTS700082: The refresh token has expired due to inactivity."
        ),
        other => panic!("expected a refused grant, got {other:?}"),
    }
}

#[tokio::test]
async fn consent_errors_need_sign_in_in_microsofts_words() {
    let consent = refresh_refused(
        400,
        json!({"error": "invalid_grant",
               "error_description": "AADSTS65001: The user or administrator has not consented to use the application.",
               "error_codes": [65001]}),
    )
    .await;
    assert!(
        matches!(
            &consent,
            SyncError::NeedsSignIn(SignInReason::CredentialRejected { detail })
                if detail.starts_with("AADSTS65001: The user or administrator has not consented")
        ),
        "{consent:?}"
    );

    let admin = refresh_refused(
        400,
        json!({"error": "invalid_grant",
               "error_description": "AADSTS90094: The grant requires admin permission.",
               "error_codes": [90094]}),
    )
    .await;
    assert!(
        matches!(
            &admin,
            SyncError::NeedsSignIn(SignInReason::AdminPolicy { detail })
                if detail == "AADSTS90094: The grant requires admin permission."
        ),
        "{admin:?}"
    );
}

#[tokio::test]
async fn a_provider_in_trouble_is_to_try_again() {
    let err = refresh_refused(
        503,
        json!({"error": "temporarily_unavailable", "error_description": "busy"}),
    )
    .await;
    assert!(matches!(err, SyncError::Api { status: 503, .. }), "{err:?}");
}

#[tokio::test]
async fn no_grant_needs_sign_in() {
    let server = MockServer::start().await;
    let (oauth, secrets) = oauth(&server);
    let err = oauth.access_token(&account()).await.unwrap_err();
    assert!(matches!(
        err,
        SyncError::NeedsSignIn(SignInReason::NoCredential)
    ));
    // A password is no grant.
    secrets
        .set(&account(), &ImapSecret::password("pw"))
        .await
        .unwrap();
    let err = oauth.access_token(&account()).await.unwrap_err();
    assert!(matches!(
        err,
        SyncError::NeedsSignIn(SignInReason::NoCredential)
    ));
}

#[tokio::test]
async fn signs_in_to_imap_with_a_refreshed_token() {
    let provider = MockServer::start().await;
    mock_token(
        &provider,
        "/ms/token",
        "refresh_token",
        200,
        refreshed("from-microsoft", 3599),
    )
    .await;
    let (oauth, secrets) = oauth(&provider);
    keep_grant(&secrets, "microsoft", "rt").await;

    let fake = ImapServer::start(ImapConfig::new().oauth_token(USER, "from-microsoft"))
        .await
        .unwrap();
    let imap = server(fake.tls_addr().unwrap(), Security::Tls);
    let settings = settings(imap.clone(), imap, microsoft());
    let secret = secrets.get(&account()).await.unwrap();
    let session = ImapSession::open(
        &connector(fake.certificate()),
        &account(),
        &settings,
        &secret,
        oauth.as_ref(),
    )
    .await
    .unwrap();
    assert!(
        fake.commands()
            .iter()
            .any(|c| c.starts_with("AUTHENTICATE XOAUTH2"))
    );
    session.logout().await.unwrap();
}
