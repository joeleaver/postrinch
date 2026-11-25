//! Google accounts and Google over IMAP share one OAuth client, so one
//! grant per address: no sign-out or refused sign-in of one kind revokes
//! the other's token, and an address is never both kinds of account.

use postrinch_model::{Account, Provider};
use postrinch_store::Store;
use postrinch_sync::google::sign_in_google;
use postrinch_sync::imap::{ImapSecret, OAuthProvider};
use postrinch_sync::{SecretStore, SyncError, check_address_free};
use wiremock::MockServer;

use crate::oauth_mock::{
    Keychains, browser, expect_revoke, keep_grant, mock_google_sign_in, no_browser,
};
use crate::support::{USER, account};

const ALL_SCOPES: &str =
    "openid email https://mail.google.com/ https://www.googleapis.com/auth/calendar";

fn already_added(err: &SyncError, as_kind: Provider) {
    match err {
        SyncError::AlreadyAdded { address, existing } => {
            assert_eq!(address, &account());
            assert_eq!(*existing, as_kind);
        }
        other => panic!("expected the address to be taken, got {other:?}"),
    }
}

#[tokio::test]
async fn signing_an_imap_account_out_keeps_a_google_accounts_grant() {
    let server = MockServer::start().await;
    expect_revoke(&server, "imap-rt", 0).await;
    let keychains = Keychains::new();
    keychains.google.set(&account(), "google-rt").unwrap();
    keep_grant(&keychains.imap, "google", "imap-rt").await;

    keychains
        .imap_oauth(&server)
        .sign_out(&account())
        .await
        .unwrap();
    assert!(keychains.imap.get(&account()).await.is_err());
    assert_eq!(
        keychains.google.get(&account()).unwrap().as_deref(),
        Some("google-rt")
    );
}

#[tokio::test]
async fn signing_a_google_account_out_keeps_an_imap_accounts_grant() {
    let server = MockServer::start().await;
    expect_revoke(&server, "google-rt", 0).await;
    let keychains = Keychains::new();
    keychains.google.set(&account(), "google-rt").unwrap();
    keep_grant(&keychains.imap, "google", "imap-rt").await;

    let auth = keychains.google_auth(&server);
    auth.sign_out(&account()).await.unwrap();
    assert_eq!(keychains.google.get(&account()).unwrap(), None);
    assert!(keychains.imap.get(&account()).await.is_ok());
}

#[tokio::test]
async fn a_grant_no_one_else_holds_is_revoked() {
    let server = MockServer::start().await;
    expect_revoke(&server, "google-rt", 1).await;
    let keychains = Keychains::new();
    keychains.google.set(&account(), "google-rt").unwrap();
    // A Microsoft grant shares nothing with Google's.
    keep_grant(&keychains.imap, "microsoft", "ms-rt").await;

    keychains
        .google_auth(&server)
        .sign_out(&account())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_refused_google_sign_in_keeps_an_imap_accounts_grant() {
    let server = MockServer::start().await;
    // Without the calendar: refused, and its token would be revoked if
    // the IMAP account did not hold one under the same grant.
    mock_google_sign_in(&server, "openid email https://mail.google.com/").await;
    expect_revoke(&server, "g-new", 0).await;
    let keychains = Keychains::new();
    keep_grant(&keychains.imap, "google", "imap-rt").await;
    let auth = keychains.google_auth(&server);

    let err = auth.sign_in(None, browser(|_| {})).await.unwrap_err();
    assert!(matches!(err, SyncError::NeedsSignIn(_)), "{err:?}");
    assert_eq!(keychains.google.get(&account()).unwrap(), None);
}

#[tokio::test]
async fn an_imap_account_is_not_added_as_a_google_account() {
    let server = MockServer::start().await;
    mock_google_sign_in(&server, ALL_SCOPES).await;
    expect_revoke(&server, "g-new", 0).await;
    let keychains = Keychains::new();
    keep_grant(&keychains.imap, "google", "imap-rt").await;
    let auth = keychains.google_auth(&server);

    // Without a hint, once Google says who signed in.
    let err = auth.sign_in(None, browser(|_| {})).await.unwrap_err();
    already_added(&err, Provider::Imap);
    assert_eq!(
        err.to_string(),
        format!("{USER} is already added as an IMAP account")
    );
    assert_eq!(keychains.google.get(&account()).unwrap(), None);
    assert!(matches!(
        keychains.imap.get(&account()).await.unwrap(),
        ImapSecret::OAuth2 { refresh_token, .. } if refresh_token == "imap-rt"
    ));

    // With the address as the hint, before the browser opens.
    let err = auth
        .sign_in(Some(&account()), no_browser)
        .await
        .unwrap_err();
    already_added(&err, Provider::Imap);
}

#[tokio::test]
async fn a_password_imap_accounts_address_refuses_and_revokes_the_new_grant() {
    let server = MockServer::start().await;
    mock_google_sign_in(&server, ALL_SCOPES).await;
    // The new grant is no one's: given back.
    expect_revoke(&server, "g-new", 1).await;
    let keychains = Keychains::new();
    let password = ImapSecret::password("app-password");
    keychains.imap.set(&account(), &password).await.unwrap();

    let auth = keychains.google_auth(&server);
    let err = auth.sign_in(None, browser(|_| {})).await.unwrap_err();
    already_added(&err, Provider::Imap);
    assert_eq!(keychains.imap.get(&account()).await.unwrap(), password);
}

#[tokio::test]
async fn a_google_account_is_not_added_as_an_imap_account() {
    let server = MockServer::start().await;
    let keychains = Keychains::new();
    keychains.google.set(&account(), "google-rt").unwrap();
    let oauth = keychains.imap_oauth(&server);

    for provider in [OAuthProvider::Google, OAuthProvider::Microsoft] {
        let err = oauth
            .sign_in(provider, &account(), no_browser)
            .await
            .unwrap_err();
        already_added(&err, Provider::Google);
        assert_eq!(
            err.to_string(),
            format!("{USER} is already added as a Google account")
        );
    }
    assert!(keychains.imap.get(&account()).await.is_err());
    assert_eq!(
        keychains.google.get(&account()).unwrap().as_deref(),
        Some("google-rt")
    );
}

#[tokio::test]
async fn the_store_refuses_an_address_of_the_other_kind() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .put_account(&Account::new(account(), Provider::Imap))
        .unwrap();

    let err = check_address_free(&store, &account(), Provider::Google)
        .await
        .unwrap_err();
    already_added(&err, Provider::Imap);
    check_address_free(&store, &account(), Provider::Imap)
        .await
        .unwrap();
    let other = postrinch_model::EmailAddress::parse("bob@example.org").unwrap();
    check_address_free(&store, &other, Provider::Google)
        .await
        .unwrap();
}

#[tokio::test]
async fn adding_a_google_account_over_a_stored_imap_account_keeps_nothing() {
    let server = MockServer::start().await;
    mock_google_sign_in(&server, ALL_SCOPES).await;
    // The IMAP account's sign-in is not in the keychain, so the new grant
    // is no one else's.
    expect_revoke(&server, "g-new", 1).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .put_account(&Account::new(account(), Provider::Imap))
        .unwrap();
    let keychains = Keychains::new();
    let auth = keychains.google_auth(&server);

    let err = sign_in_google(&auth, &store, None, browser(|_| {}))
        .await
        .unwrap_err();
    already_added(&err, Provider::Imap);
    assert_eq!(keychains.google.get(&account()).unwrap(), None);
    let kept = store.account(&account()).unwrap().unwrap();
    assert_eq!(kept.provider, Provider::Imap);

    let err = sign_in_google(&auth, &store, Some(&account()), no_browser)
        .await
        .unwrap_err();
    already_added(&err, Provider::Imap);
}
