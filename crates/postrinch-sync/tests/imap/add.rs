//! Adding an IMAP account ([`ImapSync::add_account`]): checked against
//! both fake servers before anything is kept, its password or OAuth grant
//! kept only once both took it, an account already here left as it was
//! when they did not, and what it learned kept when they did; signing in
//! again and removing it.

use std::sync::Arc;

use postrinch_model::{
    Account, AccountState, FolderRoles, Provider, Security, SignInMethod, SignInReason,
};
use postrinch_store::Store;
use postrinch_sync::imap::{
    Added, CheckFailure, ImapOAuth, ImapSecret, ImapSecrets, ImapSync, NewImapAccount, NewSignIn,
    OAuthClients, OAuthProvider, SignInAgain,
};
use postrinch_sync::{MemorySecrets, SecretStore, SyncError};
use wiremock::MockServer;

use super::check::Fakes;
use super::oauth_mock::{
    browser, clients, expect_revoke, keep_grant, kept_refresh_token, mock_google_sign_in,
};
use super::support::{PASSWORD, Tokens, account, microsoft, settings};

/// The scopes Google over IMAP asks for.
pub const MAIL_SCOPES: &str = "openid email https://mail.google.com/";

pub struct Adding {
    pub fakes: Fakes,
    pub store: Store,
    pub secrets: ImapSecrets,
    pub sync: ImapSync,
    pub oauth: ImapOAuth,
    _dir: tempfile::TempDir,
}

/// Fakes taking the password, and no OAuth provider.
pub async fn adding() -> Adding {
    adding_with(Fakes::with_passwords().await, OAuthClients::none()).await
}

pub async fn adding_with(fakes: Fakes, clients: OAuthClients) -> Adding {
    adding_over(fakes, clients, Arc::new(MemorySecrets::default())).await
}

/// As [`adding_with`], the accounts' secrets kept in `keychain`.
pub async fn adding_over(
    fakes: Fakes,
    clients: OAuthClients,
    keychain: Arc<dyn SecretStore>,
) -> Adding {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let secrets = ImapSecrets::new(keychain);
    let sync = ImapSync::new(
        store.clone(),
        fakes.connector(),
        secrets.clone(),
        Arc::new(Tokens::none()),
    );
    let oauth =
        ImapOAuth::new(clients, secrets.clone(), Arc::new(MemorySecrets::default())).unwrap();
    Adding {
        fakes,
        store,
        secrets,
        sync,
        oauth,
        _dir: dir,
    }
}

impl Adding {
    pub fn new_account(&self) -> NewImapAccount {
        let mut settings = settings(
            self.fakes.imap_server(),
            self.fakes.smtp_server(Security::Tls),
            SignInMethod::Password,
        );
        settings.roles = FolderRoles::default();
        NewImapAccount {
            address: account(),
            display_name: Some("Ada".into()),
            settings,
        }
    }

    async fn add(&self, password: &str) -> Added {
        self.add_over(&self.sync, password).await
    }

    /// Adds the account with `password` through `sync`.
    pub async fn add_over(&self, sync: &ImapSync, password: &str) -> Added {
        sync.add_account(
            &self.oauth,
            self.new_account(),
            NewSignIn::Password(ImapSecret::password(password)),
        )
        .await
        .unwrap()
    }

    /// Adds the account with Google over IMAP, as the mock browser signs
    /// in.
    pub async fn add_with_google(&self) -> Added {
        self.try_add_with_google().await.unwrap()
    }

    /// Adds the account with Google over IMAP, as the mock browser signs
    /// in, however that goes.
    pub async fn try_add_with_google(&self) -> Result<Added, SyncError> {
        let sign_in = NewSignIn::OAuth {
            provider: OAuthProvider::Google,
            open_browser: Box::new(browser(|_| {})),
            cancel: None,
            on_grant: None,
        };
        self.sync
            .add_account(&self.oauth, self.new_account(), sign_in)
            .await
    }

    /// Stores the account as signing in with `method`, with what sync
    /// learned about it: an archive folder chosen, and a provider that
    /// does not file sent mail.
    pub fn store_learned(&self, method: SignInMethod) -> Account {
        let mut stored = Account::new(account(), Provider::Imap);
        let mut settings = self.new_account().settings;
        settings.sign_in = method;
        settings.roles.archive = Some("Old".into());
        settings.server_files_sent = false;
        stored.imap = Some(settings);
        stored.display_name = Some("Ada".into());
        self.store.put_account(&stored).unwrap();
        stored
    }

    pub fn stored(&self) -> Option<Account> {
        self.store.account(&account()).unwrap()
    }
}

/// Google over IMAP at a mock server, its sign-in as the account.
pub async fn google() -> MockServer {
    let server = MockServer::start().await;
    mock_google_sign_in(&server, MAIL_SCOPES).await;
    server
}

pub fn google_sign_in() -> SignInMethod {
    OAuthProvider::Google.sign_in_method()
}

#[tokio::test]
async fn an_account_both_servers_take_is_stored_with_its_password_kept() {
    let a = adding().await;
    let Added::Account(added) = a.add(PASSWORD).await else {
        panic!("the check failed");
    };
    assert_eq!(added.provider, Provider::Imap);
    let stored = a.stored().unwrap();
    assert!(stored.is_active());
    assert_eq!(stored.display_name.as_deref(), Some("Ada"));
    assert_eq!(stored.imap, Some(a.new_account().settings));
    assert_eq!(
        a.secrets.get(&account()).await.unwrap(),
        ImapSecret::password(PASSWORD)
    );
    // It syncs with what was kept.
    a.sync.sync(&account(), Arc::new(|_| {})).await.unwrap();
}

#[tokio::test]
async fn a_refused_password_stores_and_keeps_nothing() {
    let a = adding().await;
    let Added::CheckFailed(check) = a.add("wrong").await else {
        panic!("a wrong password passed");
    };
    assert!(
        matches!(
            check.imap,
            Err(CheckFailure::SignIn(
                SignInReason::CredentialRejected { .. }
            ))
        ),
        "{check:?}"
    );
    assert!(check.to_string().contains("IMAP: it refused the sign-in"));
    assert!(a.stored().is_none());
    assert!(a.secrets.get(&account()).await.is_err(), "no password kept");
}

#[tokio::test]
async fn adding_again_changes_the_settings_and_signs_it_in() {
    let a = adding().await;
    a.add(PASSWORD).await;
    let mut stored = a.stored().unwrap();
    stored.needs_sign_in(SignInReason::NoCredential);
    a.store.put_account(&stored).unwrap();
    let Added::Account(_) = a.add(PASSWORD).await else {
        panic!("the check failed");
    };
    assert_eq!(a.stored().unwrap().state, AccountState::Active);
}

#[tokio::test]
async fn an_address_that_is_a_google_account_is_refused_first() {
    let a = adding().await;
    a.store
        .put_account(&Account::new(account(), Provider::Google))
        .unwrap();
    let result = a
        .sync
        .add_account(
            &a.oauth,
            a.new_account(),
            NewSignIn::Password(ImapSecret::password(PASSWORD)),
        )
        .await;
    assert!(
        matches!(result, Err(SyncError::AlreadyAdded { .. })),
        "{result:?}"
    );
    assert_eq!(a.fakes.imap.command_count("LOGIN"), 0, "nothing opened");
}

#[tokio::test]
async fn a_password_account_has_no_browser_sign_in() {
    let a = adding().await;
    a.add(PASSWORD).await;
    let result = a
        .sync
        .sign_in_again(
            &a.oauth,
            &account(),
            SignInAgain::Browser(Box::new(|_| Ok(()))),
        )
        .await;
    assert!(matches!(result, Err(SyncError::Config(_))), "{result:?}");
}

#[tokio::test]
async fn removing_forgets_the_password_and_the_mail() {
    let a = adding().await;
    a.add(PASSWORD).await;
    a.sync.remove_account(&a.oauth, &account()).await.unwrap();
    assert!(a.stored().is_none());
    assert!(a.secrets.get(&account()).await.is_err());
}

#[tokio::test]
async fn adding_again_keeps_what_the_person_chose_and_sync_learned() {
    let a = adding().await;
    let learned = a.store_learned(SignInMethod::SeparateSmtpPassword);
    a.secrets
        .set(&account(), &ImapSecret::password(PASSWORD))
        .await
        .unwrap();
    let Added::Account(added) = a.add(PASSWORD).await else {
        panic!("the check failed");
    };
    let settings = added.imap.unwrap();
    assert_eq!(settings.sign_in, SignInMethod::Password, "the add's");
    let kept = learned.imap.unwrap();
    assert_eq!(settings.roles, kept.roles);
    assert!(!settings.server_files_sent);
    assert_eq!(a.stored().unwrap().imap, Some(settings));
}

#[tokio::test]
async fn an_oauth_account_the_servers_refuse_is_given_back_and_nothing_kept() {
    let server = google().await;
    // Nothing else holds the grant: it goes back.
    expect_revoke(&server, "g-new", 1).await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    let Added::CheckFailed(check) = a.add_with_google().await else {
        panic!("a token the servers do not know passed");
    };
    assert!(check.imap.is_err() && check.smtp.is_err(), "{check:?}");
    assert!(a.stored().is_none());
    assert!(a.secrets.get(&account()).await.is_err(), "no grant kept");
}

#[tokio::test]
async fn oauth_over_a_password_account_the_servers_refuse_leaves_it_as_it_was() {
    let server = google().await;
    expect_revoke(&server, "g-new", 1).await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    let before = a.store_learned(SignInMethod::Password);
    let password = ImapSecret::password(PASSWORD);
    a.secrets.set(&account(), &password).await.unwrap();

    let Added::CheckFailed(_) = a.add_with_google().await else {
        panic!("a token the servers do not know passed");
    };
    assert_eq!(a.secrets.get(&account()).await.unwrap(), password);
    assert_eq!(a.stored(), Some(before));
    // It still syncs with its password.
    a.sync.sync(&account(), Arc::new(|_| {})).await.unwrap();
}

#[tokio::test]
async fn a_provider_change_the_servers_refuse_leaves_it_as_it_was() {
    let server = google().await;
    // The account's grant is Microsoft's: Google's unused one goes back.
    expect_revoke(&server, "g-new", 1).await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    let before = a.store_learned(microsoft());
    keep_grant(&a.secrets, "microsoft", "ms-old").await;

    let Added::CheckFailed(_) = a.add_with_google().await else {
        panic!("a token the servers do not know passed");
    };
    assert_eq!(kept_refresh_token(&a.secrets).await, "ms-old");
    assert_eq!(a.stored(), Some(before));
}

#[tokio::test]
async fn oauth_over_a_password_account_keeps_the_grant_and_what_was_learned() {
    let server = google().await;
    expect_revoke(&server, "g-new", 0).await;
    let fakes = Fakes::with_password_and_token("g-access").await;
    let a = adding_with(fakes, clients(&server)).await;
    let before = a.store_learned(SignInMethod::Password);
    a.secrets
        .set(&account(), &ImapSecret::password(PASSWORD))
        .await
        .unwrap();

    let Added::Account(added) = a.add_with_google().await else {
        panic!("the check failed");
    };
    assert_eq!(kept_refresh_token(&a.secrets).await, "g-new");
    let settings = added.imap.unwrap();
    assert_eq!(settings.sign_in, google_sign_in());
    let kept = before.imap.unwrap();
    assert_eq!(
        (settings.roles, settings.server_files_sent),
        (kept.roles, false)
    );
}

#[tokio::test]
async fn a_password_over_an_oauth_account_gives_the_old_grant_back() {
    let server = google().await;
    // Once the password is checked and kept, and not before.
    expect_revoke(&server, "g-old", 1).await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    a.store_learned(google_sign_in());
    keep_grant(&a.secrets, "google", "g-old").await;

    let Added::CheckFailed(_) = a.add("wrong").await else {
        panic!("a wrong password passed");
    };
    assert_eq!(kept_refresh_token(&a.secrets).await, "g-old");
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().all(|r| r.url.path() != "/g/revoke"));
    let Added::Account(added) = a.add(PASSWORD).await else {
        panic!("the check failed");
    };
    assert_eq!(added.imap.unwrap().sign_in, SignInMethod::Password);
    assert_eq!(
        a.secrets.get(&account()).await.unwrap(),
        ImapSecret::password(PASSWORD)
    );
}
