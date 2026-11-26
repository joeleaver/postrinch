//! Adding an IMAP account over the secret it had
//! ([`ImapSync::add_account`]): a Google grant over a Google grant, a
//! password while a refresh rotates the grant it replaces, a checked
//! secret that cannot be kept, and signing in again.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use postrinch_model::{EmailAddress, SignInMethod, SignInReason};
use postrinch_sync::imap::{AccessTokens, Added, ImapSecret, NewSignIn, SignInAgain};
use postrinch_sync::{MemorySecrets, SecretStore, SyncError, SyncResult};
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::add::{Adding, MAIL_SCOPES, adding, adding_over, adding_with, google, google_sign_in};
use super::check::Fakes;
use super::oauth_mock::{browser, clients, expect_revoke, keep_grant, kept_refresh_token};
use super::support::{PASSWORD, account};

impl Adding {
    /// Adds the account with `password`, however that goes.
    async fn try_add(&self, password: &str) -> Result<Added, SyncError> {
        let sign_in = NewSignIn::Password(ImapSecret::password(password));
        self.sync
            .add_account(&self.oauth, self.new_account(), sign_in)
            .await
    }
}

/// Fakes taking Google's access token, the account a Google grant `g-old`.
async fn over_a_google_grant(server: &MockServer) -> Adding {
    let fakes = Fakes::with_password_and_token("g-access").await;
    let a = adding_with(fakes, clients(server)).await;
    a.store_learned(google_sign_in());
    keep_grant(&a.secrets, "google", "g-old").await;
    a
}

#[tokio::test]
async fn google_over_a_google_grant_keeps_the_new_grant_and_revokes_nothing() {
    let server = google().await;
    // One token revoked would withdraw the whole grant, the new one too.
    expect_revoke(&server, "g-old", 0).await;
    expect_revoke(&server, "g-new", 0).await;
    let a = over_a_google_grant(&server).await;

    let Added::Account(added) = a.add_with_google().await else {
        panic!("the check failed");
    };
    assert_eq!(added.imap.unwrap().sign_in, google_sign_in());
    assert_eq!(kept_refresh_token(&a.secrets).await, "g-new");
    // The new grant's access token is the one kept for the account.
    assert_eq!(a.oauth.access_token(&account()).await.unwrap(), "g-access");
}

#[tokio::test]
async fn google_over_a_google_grant_the_servers_refuse_revokes_nothing() {
    let server = google().await;
    expect_revoke(&server, "g-old", 0).await;
    expect_revoke(&server, "g-new", 0).await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    let before = a.store_learned(google_sign_in());
    keep_grant(&a.secrets, "google", "g-old").await;

    let Added::CheckFailed(_) = a.add_with_google().await else {
        panic!("a token the servers do not know passed");
    };
    assert_eq!(kept_refresh_token(&a.secrets).await, "g-old");
    assert_eq!(a.stored(), Some(before));
}

/// Waits until `server` has been asked for a token: the refresh asking
/// holds the account's token slot until it is answered.
async fn refresh_asked(server: &MockServer) {
    loop {
        let requests = server.received_requests().await.unwrap();
        if requests.iter().any(|r| r.url.path() == "/g/token") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_password_added_during_a_refresh_replaces_the_rotated_grant() {
    let server = google().await;
    // The refresh is answered late, with a rotated refresh token.
    Mock::given(method("POST"))
        .and(path("/g/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=g-old"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "access_token": "g-fresh", "expires_in": 3599,
                    "refresh_token": "g-rotated", "scope": MAIL_SCOPES
                }))
                .set_delay(Duration::from_secs(1)),
        )
        .expect(1)
        .mount(&server)
        .await;
    // The grant the password replaced is the rotated one.
    expect_revoke(&server, "g-rotated", 1).await;
    expect_revoke(&server, "g-old", 0).await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    a.store_learned(google_sign_in());
    keep_grant(&a.secrets, "google", "g-old").await;

    let address = account();
    let refresh = a.oauth.access_token(&address);
    let add = async {
        refresh_asked(&server).await;
        a.try_add(PASSWORD).await
    };
    let (token, added) = tokio::join!(refresh, add);
    assert_eq!(token.unwrap(), "g-fresh");
    let Added::Account(added) = added.unwrap() else {
        panic!("the check failed");
    };
    assert_eq!(added.imap.unwrap().sign_in, SignInMethod::Password);
    assert_eq!(
        a.secrets.get(&account()).await.unwrap(),
        ImapSecret::password(PASSWORD),
        "no rotated token was kept over the password"
    );
    let token = a.oauth.access_token(&account()).await;
    assert!(
        matches!(
            token,
            Err(SyncError::NeedsSignIn(SignInReason::NoCredential))
        ),
        "the refreshed access token was forgotten: {token:?}"
    );
}

#[tokio::test]
async fn a_grant_the_store_cannot_take_is_given_back_and_the_password_stays() {
    let server = google().await;
    expect_revoke(&server, "g-new", 1).await;
    let fakes = Fakes::with_password_and_token("g-access").await;
    let a = adding_with(fakes, clients(&server)).await;
    let before = a.store_learned(SignInMethod::Password);
    let password = ImapSecret::password(PASSWORD);
    a.secrets.set(&account(), &password).await.unwrap();
    a.store.fail_updates("Account", 1);

    let result = a.try_add_with_google().await;
    assert!(matches!(result, Err(SyncError::Store(_))), "{result:?}");
    assert_eq!(a.secrets.get(&account()).await.unwrap(), password);
    assert_eq!(a.stored(), Some(before));
}

#[tokio::test]
async fn a_google_grant_over_a_google_grant_the_store_cannot_take_revokes_nothing() {
    let server = google().await;
    expect_revoke(&server, "g-old", 0).await;
    expect_revoke(&server, "g-new", 0).await;
    let a = over_a_google_grant(&server).await;
    a.store.fail_updates("Account", 1);

    let result = a.try_add_with_google().await;
    assert!(matches!(result, Err(SyncError::Store(_))), "{result:?}");
    assert_eq!(kept_refresh_token(&a.secrets).await, "g-old");
}

#[tokio::test]
async fn a_password_the_store_cannot_take_leaves_the_grant_it_was_to_replace() {
    let server = google().await;
    expect_revoke(&server, "g-old", 0).await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    let before = a.store_learned(google_sign_in());
    keep_grant(&a.secrets, "google", "g-old").await;
    a.store.fail_updates("Account", 1);

    let result = a.try_add(PASSWORD).await;
    assert!(matches!(result, Err(SyncError::Store(_))), "{result:?}");
    assert_eq!(kept_refresh_token(&a.secrets).await, "g-old");
    assert_eq!(a.stored(), Some(before));
}

#[tokio::test]
async fn an_account_without_a_secret_the_store_cannot_take_keeps_no_password() {
    let a = adding().await;
    // Its secret is gone (the keychain was cleared): it has to sign in.
    let before = a.store_learned(SignInMethod::Password);
    a.store.fail_updates("Account", 1);

    let result = a.try_add(PASSWORD).await;
    assert!(matches!(result, Err(SyncError::Store(_))), "{result:?}");
    assert_eq!(a.stored(), Some(before));
    assert!(a.secrets.get(&account()).await.is_err(), "no password kept");
}

/// A keychain that, once told to, takes no new secret.
#[derive(Default)]
struct Refusing {
    inner: MemorySecrets,
    refuse: AtomicBool,
}

impl SecretStore for Refusing {
    fn get(&self, account: &EmailAddress) -> SyncResult<Option<String>> {
        self.inner.get(account)
    }

    fn set(&self, account: &EmailAddress, secret: &str) -> SyncResult<()> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(SyncError::Io(std::io::Error::other(
                "the keychain is locked",
            )));
        }
        self.inner.set(account, secret)
    }

    fn delete(&self, account: &EmailAddress) -> SyncResult<()> {
        self.inner.delete(account)
    }
}

#[tokio::test]
async fn a_grant_the_keychain_will_not_take_is_given_back() {
    let server = google().await;
    expect_revoke(&server, "g-new", 1).await;
    let keychain = Arc::new(Refusing::default());
    let fakes = Fakes::with_password_and_token("g-access").await;
    let a = adding_over(fakes, clients(&server), keychain.clone()).await;
    let before = a.store_learned(SignInMethod::Password);
    let password = ImapSecret::password(PASSWORD);
    a.secrets.set(&account(), &password).await.unwrap();
    keychain.refuse.store(true, Ordering::SeqCst);

    let result = a.try_add_with_google().await;
    assert!(matches!(result, Err(SyncError::Io(_))), "{result:?}");
    assert_eq!(a.secrets.get(&account()).await.unwrap(), password);
    assert_eq!(a.stored(), Some(before));
}

#[tokio::test]
async fn signing_in_again_keeps_what_sync_learned() {
    let server = google().await;
    expect_revoke(&server, "g-old", 0).await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    let mut learned = a.store_learned(google_sign_in());
    learned.needs_sign_in(SignInReason::NoCredential);
    a.store.put_account(&learned).unwrap();
    keep_grant(&a.secrets, "google", "g-old").await;

    let signed_in = a
        .sync
        .sign_in_again(
            &a.oauth,
            &account(),
            SignInAgain::Browser(Box::new(browser(|_| {}))),
        )
        .await
        .unwrap();
    let Added::Account(signed_in) = signed_in else {
        panic!("{signed_in:?}")
    };
    let signed_in = *signed_in;
    assert!(signed_in.is_active());
    assert_eq!(signed_in.imap, learned.imap, "what was learned stays");
    assert_eq!(a.stored(), Some(signed_in));
    assert_eq!(kept_refresh_token(&a.secrets).await, "g-new");
}
