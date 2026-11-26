//! OAuth when things go wrong around it: a provider that never answers, a
//! browser that reaches the redirect on either loopback after opening an
//! idle connection to the other, and a sign-in racing a refresh.

use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use postrinch_sync::imap::{AccessTokens, ImapOAuth, OAuthProvider};
use postrinch_sync::{OAuthTimeouts, SyncError};
use postrinch_sync_core::SecretStore;
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::oauth_mock::{
    Keychains, MICROSOFT_SCOPES, browser, browser_via, clients, keep_grant, kept_refresh_token,
    microsoft_grant, mock_token, oauth, param,
};
use crate::support::{USER, account};

/// Short, so a test of a provider that never answers is quick.
const SHORT: OAuthTimeouts = OAuthTimeouts {
    connect: Duration::from_millis(500),
    request: Duration::from_millis(300),
};

/// Longer than any timeout here: a call that takes this long hung.
const HUNG: Duration = Duration::from_secs(10);

/// Answers `POST <at>` after a minute.
async fn never_answer(server: &MockServer, at: &str) {
    Mock::given(method("POST"))
        .and(path(at))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "late", "expires_in": 3599}))
                .set_delay(Duration::from_secs(60)),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_token_endpoint_that_never_answers_fails_the_refresh_and_frees_the_account() {
    let server = MockServer::start().await;
    never_answer(&server, "/ms/token").await;
    let keychains = Keychains::new();
    keep_grant(&keychains.imap, "microsoft", "rt").await;
    let oauth = ImapOAuth::new(
        clients(&server),
        keychains.imap.clone(),
        keychains.google.clone(),
    )
    .unwrap()
    .with_timeouts(SHORT)
    .unwrap();

    // Twice: the first refresh let go of the account's lock.
    for _ in 0..2 {
        let err = tokio::time::timeout(HUNG, oauth.access_token(&account()))
            .await
            .expect("the refresh hung")
            .unwrap_err();
        assert!(matches!(err, SyncError::Http(_)), "{err:?}");
    }
}

#[tokio::test]
async fn google_requests_that_never_answer_fail() {
    let server = MockServer::start().await;
    never_answer(&server, "/g/token").await;
    never_answer(&server, "/g/revoke").await;
    let keychains = Keychains::new();
    keychains.google.set(&account(), "rt").unwrap();
    let auth = keychains.google_auth(&server).with_timeouts(SHORT).unwrap();

    let err = tokio::time::timeout(HUNG, auth.access_token(&account()))
        .await
        .expect("the refresh hung")
        .unwrap_err();
    assert!(matches!(err, SyncError::Http(_)), "{err:?}");

    // A revocation that never answers does not hold up signing out.
    tokio::time::timeout(HUNG, auth.sign_out(&account()))
        .await
        .expect("signing out hung")
        .unwrap();
    assert_eq!(keychains.google.get(&account()).unwrap(), None);
}

fn has_ipv6_loopback() -> bool {
    std::net::TcpListener::bind(("::1", 0)).is_ok()
}

/// Sign in with Microsoft (a `localhost` redirect), the browser reaching
/// it at `address` after an idle connection to `idle_at`.
async fn sign_in_through(address: &'static str, idle_at: Option<&'static str>) {
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
    let idle = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::clone(&idle);

    let signed_in = oauth
        .sign_in(
            OAuthProvider::Microsoft,
            &account(),
            browser_via(Some(address), move |page| {
                let redirect = url::Url::parse(&param(page, "redirect_uri").unwrap()).unwrap();
                assert_eq!(redirect.host_str(), Some("localhost"));
                if let Some(idle_at) = idle_at {
                    let port = redirect.port().unwrap();
                    let stream = TcpStream::connect(format!("{idle_at}:{port}")).unwrap();
                    opened.lock().unwrap().push(stream);
                }
            }),
        )
        .await
        .unwrap();

    assert_eq!(signed_in.address, account());
    assert_eq!(kept_refresh_token(&secrets).await, "ms-refresh");
    assert_eq!(oauth.access_token(&account()).await.unwrap(), "ms-access");
    assert_eq!(idle.lock().unwrap().len(), usize::from(idle_at.is_some()));
}

#[tokio::test]
async fn signs_in_through_the_ipv4_loopback() {
    let idle = has_ipv6_loopback().then_some("[::1]");
    sign_in_through("127.0.0.1", idle).await;
}

#[tokio::test]
async fn signs_in_through_the_ipv6_loopback() {
    if !has_ipv6_loopback() {
        eprintln!("no IPv6 loopback on this host: the redirect is heard on 127.0.0.1 alone");
        return;
    }
    sign_in_through("[::1]", Some("127.0.0.1")).await;
}

#[tokio::test]
async fn a_sign_in_racing_a_refresh_keeps_the_new_grant() {
    let server = MockServer::start().await;
    // The old grant's refresh is slow, and rotates its refresh token.
    Mock::given(method("POST"))
        .and(path("/ms/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=old-rt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "access_token": "old-access", "expires_in": 3599,
                    "refresh_token": "old-rotated", "scope": MICROSOFT_SCOPES
                }))
                .set_delay(Duration::from_millis(500)),
        )
        .expect(1)
        .mount(&server)
        .await;
    mock_token(
        &server,
        "/ms/token",
        "authorization_code",
        200,
        microsoft_grant(USER),
    )
    .await;
    let (oauth, secrets) = oauth(&server);
    keep_grant(&secrets, "microsoft", "old-rt").await;

    let refreshing = {
        let oauth = Arc::clone(&oauth);
        tokio::spawn(async move { oauth.access_token(&account()).await })
    };
    // The refresh is under way, holding the account's token.
    tokio::time::sleep(Duration::from_millis(100)).await;
    oauth
        .sign_in(OAuthProvider::Microsoft, &account(), browser(|_| {}))
        .await
        .unwrap();

    assert_eq!(refreshing.await.unwrap().unwrap(), "old-access");
    // The rotated old token did not overwrite the new grant, nor its
    // access token the sign-in's.
    assert_eq!(kept_refresh_token(&secrets).await, "ms-refresh");
    assert_eq!(oauth.access_token(&account()).await.unwrap(), "ms-access");
}
