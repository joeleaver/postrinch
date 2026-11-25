//! An IMAP account's settings page, through [`ImapSync`]: its servers
//! changed only once both take the kept password, signing in again with a
//! password, the folders it offers for each role and the roles the person
//! picks, and the person's say on whether the provider files sent mail.

use std::sync::Arc;

use postrinch_model::{
    AccountState, FolderRole, FolderRoles, Security, SignInMethod, SignInReason, SystemLabel,
};
use postrinch_sync::SyncError;
use postrinch_sync::imap::{
    Added, ImapSecret, NewSignIn, OAuthProvider, SignInAgain, sign_in_cancel,
};

use super::add::{Adding, adding, adding_with, google};
use super::check::Fakes;
use super::oauth_mock::clients;
use super::support::{PASSWORD, USER, account, closed_port, microsoft, server};

/// An account added with the password, then stored with what sync
/// learned about it (an archive folder, a provider that does not file
/// sent mail).
async fn added() -> Adding {
    let a = adding().await;
    let Added::Account(_) = a.add_over(&a.sync, PASSWORD).await else {
        panic!("the check failed");
    };
    a.store_learned(SignInMethod::Password);
    a
}

#[tokio::test]
async fn servers_that_fail_the_check_change_nothing() {
    let a = added().await;
    let before = a.stored();
    let mut settings = a.new_account().settings;
    settings.smtp = server(closed_port().await, Security::Tls);
    let Added::CheckFailed(check) = a.sync.change_servers(&account(), settings).await.unwrap()
    else {
        panic!("a closed port passed");
    };
    assert!(check.imap.is_ok() && check.smtp.is_err(), "{check:?}");
    assert_eq!(a.stored(), before);
}

#[tokio::test]
async fn servers_that_pass_are_kept_with_what_was_learned() {
    let a = added().await;
    let mut settings = a.new_account().settings;
    settings.smtp = a.fakes.smtp_server(Security::StartTls);
    // What the servers panel does not change is not taken from it.
    settings.sign_in = microsoft();
    settings.roles = FolderRoles::default();
    settings.server_files_sent = true;
    let Added::Account(changed) = a.sync.change_servers(&account(), settings).await.unwrap() else {
        panic!("the check failed");
    };
    let imap = changed.imap.clone().unwrap();
    assert_eq!(imap.smtp.security, Security::StartTls);
    assert_eq!(imap.sign_in, SignInMethod::Password);
    assert_eq!(imap.roles.archive.as_deref(), Some("Old"));
    assert!(!imap.server_files_sent);
    assert_eq!(a.stored(), Some(*changed));
    a.sync.sync(&account(), Arc::new(|_| {})).await.unwrap();
}

#[tokio::test]
async fn a_password_signs_in_again_once_both_servers_take_it() {
    let a = added().await;
    let mut stored = a.stored().unwrap();
    stored.needs_sign_in(SignInReason::NoCredential);
    a.store.put_account(&stored).unwrap();

    let wrong = SignInAgain::Password(ImapSecret::password("wrong"));
    let Added::CheckFailed(_) = a
        .sync
        .sign_in_again(&a.oauth, &account(), wrong)
        .await
        .unwrap()
    else {
        panic!("a wrong password passed");
    };
    assert_eq!(a.stored(), Some(stored.clone()), "nothing changed");

    let right = SignInAgain::Password(ImapSecret::password(PASSWORD));
    let Added::Account(signed_in) = a
        .sync
        .sign_in_again(&a.oauth, &account(), right)
        .await
        .unwrap()
    else {
        panic!("the check failed");
    };
    assert_eq!(signed_in.state, AccountState::Active);
    assert_eq!(signed_in.imap, stored.imap, "what was learned stays");
}

#[tokio::test]
async fn an_oauth_account_does_not_sign_in_with_a_password() {
    let a = added().await;
    a.store_learned(microsoft());
    let result = a
        .sync
        .sign_in_again(
            &a.oauth,
            &account(),
            SignInAgain::Password(ImapSecret::password(PASSWORD)),
        )
        .await;
    assert!(matches!(result, Err(SyncError::Config(_))), "{result:?}");
    assert_eq!(
        a.secrets.get(&account()).await.unwrap(),
        ImapSecret::password(PASSWORD),
        "nothing kept"
    );
}

#[tokio::test]
async fn the_folders_chosen_for_roles_take_them_at_the_next_pass() {
    let a = added().await;
    a.store_learned(SignInMethod::Password);
    let user = a.fakes.imap.user(USER);
    user.create_folder("Copies").unwrap();
    user.create_folder("Entw&APw-rfe").unwrap();
    a.sync.sync(&account(), Arc::new(|_| {})).await.unwrap();

    let role = |folders: &[postrinch_sync::imap::AccountFolder], name: &str| {
        folders.iter().find(|f| f.name == name).unwrap().role
    };
    let folders = a.sync.account_folders(&account()).await.unwrap();
    let shown: Vec<&str> = folders.iter().map(|f| f.shown.as_str()).collect();
    assert_eq!(shown, ["Copies", "Entwürfe", "Sent"], "no inbox, by name");
    let sent = Some(FolderRole::System(SystemLabel::Sent));
    assert_eq!(role(&folders, "Sent"), sent);

    let roles = FolderRoles {
        sent: Some("Copies".into()),
        drafts: Some("Entw&APw-rfe".into()),
        ..FolderRoles::default()
    };
    let changed = a.sync.set_folder_roles(&account(), roles.clone()).await;
    assert_eq!(changed.unwrap().imap.unwrap().roles, roles);
    a.sync.sync(&account(), Arc::new(|_| {})).await.unwrap();
    let folders = a.sync.account_folders(&account()).await.unwrap();
    // Each folder once, under its new label at once: not also its old
    // one until a second pass.
    let shown: Vec<&str> = folders.iter().map(|f| f.shown.as_str()).collect();
    assert_eq!(shown, ["Copies", "Entwürfe", "Sent"]);
    let labels = a.store.labels(&account()).unwrap();
    let copies = labels.iter().filter(|l| l.name == "Copies").count();
    assert_eq!(copies, 1, "{labels:?}");
    assert_eq!(role(&folders, "Copies"), sent);
    assert_eq!(role(&folders, "Sent"), None);
    assert_eq!(
        role(&folders, "Entw&APw-rfe"),
        Some(FolderRole::System(SystemLabel::Drafts))
    );
    let stored = a.stored().unwrap().imap.unwrap();
    assert!(!stored.server_files_sent, "what was learned stays");
}

#[tokio::test]
async fn the_person_says_whether_the_provider_files_sent_mail() {
    let a = added().await;
    let chosen = a
        .sync
        .set_server_files_sent(&account(), Some(true))
        .await
        .unwrap();
    assert!(chosen.server_files_sent && chosen.server_files_sent_chosen);
    let stored = a.stored().unwrap().imap.unwrap();
    assert_eq!(stored, chosen);
    assert_eq!(stored.roles.archive.as_deref(), Some("Old"));

    let back = a.sync.set_server_files_sent(&account(), None).await;
    assert!(!back.unwrap().server_files_sent_chosen);
}

#[tokio::test]
async fn a_browser_sign_in_cancelled_while_it_waits_adds_nothing() {
    let server = google().await;
    let a = adding_with(Fakes::with_passwords().await, clients(&server)).await;
    let (canceller, cancel) = sign_in_cancel();
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel::<()>();
    // The browser opens, and the person never comes back from it.
    let open_browser = Box::new(move |_: &str| {
        opened_tx.send(()).unwrap();
        Ok(())
    });
    let sign_in = NewSignIn::OAuth {
        provider: OAuthProvider::Google,
        open_browser,
        cancel: Some(cancel),
    };
    let adding = a.sync.add_account(&a.oauth, a.new_account(), sign_in);
    let cancelling = async {
        opened_rx.await.unwrap();
        canceller.cancel();
    };
    let (result, ()) = tokio::join!(adding, cancelling);
    assert!(matches!(result, Err(SyncError::SignIn(_))), "{result:?}");
    assert!(a.stored().is_none());
    assert!(a.secrets.get(&account()).await.is_err(), "nothing kept");
}

#[tokio::test]
async fn settings_need_an_imap_account() {
    let a = adding().await;
    let roles = a
        .sync
        .set_folder_roles(&account(), FolderRoles::default())
        .await;
    assert!(matches!(roles, Err(SyncError::Config(_))), "{roles:?}");
    assert!(a.sync.account_folders(&account()).await.is_err());
}
