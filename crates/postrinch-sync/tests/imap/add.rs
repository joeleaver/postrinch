//! Adding an IMAP account ([`ImapSync::add_account`]): checked against
//! both fake servers before anything is kept, its password kept only
//! once both took it; signing in again and removing it.

use std::sync::Arc;

use postrinch_model::{
    Account, AccountState, FolderRoles, Provider, Security, SignInMethod, SignInReason,
};
use postrinch_store::Store;
use postrinch_sync::imap::{
    Added, CheckFailure, ImapOAuth, ImapSecret, ImapSecrets, ImapSync, NewImapAccount, NewSignIn,
    OAuthClients,
};
use postrinch_sync::{MemorySecrets, SyncError};

use super::check::Fakes;
use super::support::{PASSWORD, Tokens, account, settings};

struct Adding {
    fakes: Fakes,
    store: Store,
    secrets: ImapSecrets,
    sync: ImapSync,
    oauth: ImapOAuth,
    _dir: tempfile::TempDir,
}

async fn adding() -> Adding {
    let fakes = Fakes::with_passwords().await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
    let sync = ImapSync::new(
        store.clone(),
        fakes.connector(),
        secrets.clone(),
        Arc::new(Tokens::none()),
    );
    let oauth = ImapOAuth::new(
        OAuthClients::none(),
        secrets.clone(),
        Arc::new(MemorySecrets::default()),
    )
    .unwrap();
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
    fn new_account(&self) -> NewImapAccount {
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
        self.sync
            .add_account(
                self.new_account(),
                NewSignIn::Password(ImapSecret::password(password)),
            )
            .await
            .unwrap()
    }

    fn stored(&self) -> Option<Account> {
        self.store.account(&account()).unwrap()
    }
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
        .sign_in_again(&a.oauth, &account(), Box::new(|_| Ok(())))
        .await;
    assert!(matches!(result, Err(SyncError::SignIn(_))), "{result:?}");
}

#[tokio::test]
async fn removing_forgets_the_password_and_the_mail() {
    let a = adding().await;
    a.add(PASSWORD).await;
    a.sync.remove_account(&a.oauth, &account()).await.unwrap();
    assert!(a.stored().is_none());
    assert!(a.secrets.get(&account()).await.is_err());
}
