//! A Google account and Google over IMAP signing in for one address at
//! about the same time, or a keychain Postrinch cannot read: the address
//! is never both kinds of account, and no grant the other kind may hold
//! is revoked (docs/imap.md, "Credentials").

use std::sync::Arc;

use postrinch_model::Provider;
use postrinch_sync::SyncError;
use postrinch_sync::imap::{ImapSecrets, OAuthProvider};
use postrinch_sync_core::{MemorySecrets, SecretStore};
use wiremock::MockServer;

use crate::add::MAIL_SCOPES;
use crate::oauth_mock::{
    Keychains, browser, expect_revoke, kept_refresh_token, mock_google_sign_in,
};
use crate::support::account;

#[tokio::test]
async fn a_google_account_added_while_the_browser_is_open_is_not_signed_in_over() {
    let server = MockServer::start().await;
    mock_google_sign_in(&server, MAIL_SCOPES).await;
    // The Google account holds the same grant: not revoked.
    expect_revoke(&server, "g-new", 0).await;
    let keychains = Keychains::new();
    let oauth = keychains.imap_oauth(&server);

    let google = keychains.google.clone();
    let opened = browser(move |_| google.set(&account(), "google-rt").unwrap());
    let err = oauth
        .sign_in(OAuthProvider::Google, &account(), opened)
        .await
        .unwrap_err();
    match err {
        SyncError::AlreadyAdded { existing, .. } => assert_eq!(existing, Provider::Google),
        other => panic!("expected the address to be taken, got {other:?}"),
    }
    assert!(
        keychains.imap.get(&account()).await.is_err(),
        "nothing kept"
    );
    assert_eq!(
        keychains.google.get(&account()).unwrap().as_deref(),
        Some("google-rt")
    );
}

#[tokio::test]
async fn a_refused_google_sign_in_keeps_a_grant_an_imap_sign_in_is_making() {
    let server = MockServer::start().await;
    mock_google_sign_in(&server, MAIL_SCOPES).await;
    // Refused for want of the calendar while the IMAP sign-in's browser
    // is open: its token may be the IMAP one's grant, so not revoked.
    expect_revoke(&server, "g-new", 0).await;
    let keychains = Keychains::new();
    let oauth = keychains.imap_oauth(&server);
    let (page_tx, page_rx) = tokio::sync::oneshot::channel::<String>();
    let imap = tokio::spawn({
        let oauth = Arc::clone(&oauth);
        async move {
            let open = move |page: &str| {
                page_tx.send(page.to_owned()).unwrap();
                Ok(())
            };
            oauth.sign_in(OAuthProvider::Google, &account(), open).await
        }
    });
    let page = page_rx.await.unwrap();

    let auth = keychains.google_auth(&server);
    let err = auth.sign_in(None, browser(|_| {})).await.unwrap_err();
    assert!(matches!(err, SyncError::NeedsSignIn(_)), "{err:?}");

    // The IMAP sign-in's browser comes back, and its grant is kept.
    browser(|_| {})(&page).unwrap();
    imap.await.unwrap().unwrap();
    assert_eq!(kept_refresh_token(&keychains.imap).await, "g-new");
}

#[tokio::test]
async fn an_imap_sign_in_postrinch_cannot_read_is_not_revoked_under() {
    let server = MockServer::start().await;
    expect_revoke(&server, "google-rt", 0).await;
    let raw = Arc::new(MemorySecrets::default());
    raw.set(&account(), "not a sign-in Postrinch reads")
        .unwrap();
    let keychains = Keychains {
        imap: ImapSecrets::new(raw.clone()),
        google: Arc::new(MemorySecrets::default()),
    };
    keychains.google.set(&account(), "google-rt").unwrap();

    keychains
        .google_auth(&server)
        .sign_out(&account())
        .await
        .unwrap();
    assert_eq!(keychains.google.get(&account()).unwrap(), None);
    assert!(raw.get(&account()).unwrap().is_some(), "left as it was");
}
