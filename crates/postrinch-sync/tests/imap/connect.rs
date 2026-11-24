//! Reaching an IMAP server and signing in with a password.

use postrinch_mailtest::{ImapConfig, ImapServer, Mechanism, SpecialUse as FakeSpecialUse};
use postrinch_model::{Security, SignInMethod, SignInReason};
use postrinch_sync::SyncError;
use postrinch_sync::imap::{Connector, ImapSecret, ImapSession, SpecialUse, TlsConfig};

use crate::support::{
    PASSWORD, Tokens, USER, account, closed_port, connector, microsoft, rejected_detail, server,
    settings,
};

async fn open(
    fake: &ImapServer,
    security: Security,
    secret: &ImapSecret,
) -> postrinch_sync::SyncResult<ImapSession> {
    let addr = match security {
        Security::Tls => fake.tls_addr().unwrap(),
        Security::StartTls => fake.starttls_addr().unwrap(),
    };
    let imap = server(addr, security);
    let settings = settings(imap.clone(), imap, SignInMethod::Password);
    let connector = connector(fake.certificate());
    ImapSession::open(&connector, &account(), &settings, secret, &Tokens::none()).await
}

fn password() -> ImapSecret {
    ImapSecret::password(PASSWORD)
}

#[tokio::test]
async fn signs_in_over_tls_and_lists_folders() {
    let fake = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
        .await
        .unwrap();
    fake.user(USER).create_standard_folders().unwrap();
    fake.user(USER).create_noselect_folder("Projects").unwrap();

    let mut session = open(&fake, Security::Tls, &password()).await.unwrap();
    let caps = session.capabilities();
    assert!(caps.condstore() && caps.qresync() && caps.move_command());
    assert!(caps.uidplus() && caps.special_use() && caps.idle() && caps.enable());
    // PLAIN is preferred to LOGIN where offered.
    assert_eq!(fake.command_count("AUTHENTICATE"), 1);
    assert_eq!(fake.command_count("LOGIN"), 0);

    let folders = session.list_folders().await.unwrap();
    assert!(
        fake.commands()
            .iter()
            .any(|c| c == "LIST \"\" * RETURN (SPECIAL-USE)")
    );
    let role = |name: &str| folders.iter().find(|f| f.name == name).unwrap().special_use;
    assert_eq!(role("INBOX"), None);
    assert_eq!(role("Sent"), Some(SpecialUse::Sent));
    assert_eq!(role("Archive"), Some(SpecialUse::Archive));
    assert_eq!(role("Junk"), Some(SpecialUse::Junk));
    let projects = folders.iter().find(|f| f.name == "Projects").unwrap();
    assert!(!projects.selectable);
    assert!(folders.iter().all(|f| f.delimiter == Some('/')));
    session.logout().await.unwrap();
}

#[tokio::test]
async fn signs_in_after_starttls() {
    let config = ImapConfig::new()
        .implicit_tls(false)
        .starttls(true)
        .user(USER, PASSWORD);
    let fake = ImapServer::start(config).await.unwrap();
    let session = open(&fake, Security::StartTls, &password()).await.unwrap();
    assert!(session.capabilities().idle());
    let commands = fake.commands();
    let starttls = commands.iter().position(|c| c == "STARTTLS").unwrap();
    let sign_in = commands
        .iter()
        .position(|c| c.starts_with("AUTHENTICATE"))
        .unwrap();
    assert!(starttls < sign_in, "{commands:?}");
    // The capabilities are asked again over TLS (they now offer AUTH=).
    assert!(
        commands[starttls..sign_in]
            .iter()
            .any(|c| c == "CAPABILITY")
    );
    session.logout().await.unwrap();
}

#[tokio::test]
async fn uses_login_where_plain_is_not_offered() {
    let config = ImapConfig::new()
        .mechanisms(&[Mechanism::XOAuth2])
        .user(USER, PASSWORD);
    let fake = ImapServer::start(config).await.unwrap();
    let session = open(&fake, Security::Tls, &password()).await.unwrap();
    assert_eq!(fake.command_count("LOGIN"), 1);
    assert_eq!(fake.command_count("AUTHENTICATE"), 0);
    session.logout().await.unwrap();
}

#[tokio::test]
async fn a_wrong_password_needs_sign_in_in_the_servers_words() {
    let fake = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
        .await
        .unwrap();
    let detail = rejected_detail(open(&fake, Security::Tls, &ImapSecret::password("nope")).await);
    assert_eq!(detail, "Invalid credentials");

    // LOGIN's refusal reads the same.
    let config = ImapConfig::new().mechanisms(&[]).user(USER, PASSWORD);
    let fake = ImapServer::start(config).await.unwrap();
    let detail = rejected_detail(open(&fake, Security::Tls, &ImapSecret::password("nope")).await);
    assert_eq!(detail, "Invalid credentials");
    assert_eq!(fake.command_count("LOGIN"), 1);
}

#[tokio::test]
async fn a_server_without_starttls_gets_no_credentials() {
    let config = ImapConfig::new()
        .implicit_tls(false)
        .starttls(true)
        .offer_starttls(false)
        .user(USER, PASSWORD);
    let fake = ImapServer::start(config).await.unwrap();
    let err = open(&fake, Security::StartTls, &password())
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Insecure { .. }), "{err:?}");
    assert_eq!(fake.command_count("LOGIN"), 0);
    assert_eq!(fake.command_count("AUTHENTICATE"), 0);
    assert_eq!(fake.command_count("STARTTLS"), 0);
}

#[tokio::test]
async fn a_certificate_the_platform_does_not_trust_is_refused() {
    let fake = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
        .await
        .unwrap();
    let imap = server(fake.tls_addr().unwrap(), Security::Tls);
    let settings = settings(imap.clone(), imap, SignInMethod::Password);
    let connector = Connector::new(TlsConfig::platform().unwrap());
    let err = ImapSession::open(
        &connector,
        &account(),
        &settings,
        &password(),
        &Tokens::none(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, SyncError::Tls { .. }), "{err:?}");
    assert!(fake.commands().is_empty());
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    let fake = ImapServer::start(ImapConfig::new()).await.unwrap();
    let imap = server(closed_port().await, Security::Tls);
    let settings = settings(imap.clone(), imap, SignInMethod::Password);
    let err = ImapSession::open(
        &connector(fake.certificate()),
        &account(),
        &settings,
        &password(),
        &Tokens::none(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, SyncError::Unreachable { .. }), "{err:?}");
}

#[tokio::test]
async fn a_secret_of_another_kind_than_the_settings_is_no_credential() {
    let fake = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
        .await
        .unwrap();
    let imap = server(fake.tls_addr().unwrap(), Security::Tls);
    let settings = settings(imap.clone(), imap, microsoft());
    let err = ImapSession::open(
        &connector(fake.certificate()),
        &account(),
        &settings,
        &password(),
        &Tokens::none(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, SyncError::NeedsSignIn(SignInReason::NoCredential)),
        "{err:?}"
    );
    assert!(fake.commands().is_empty());
}

#[tokio::test]
async fn special_use_without_list_extended_lists_plainly() {
    use postrinch_mailtest::Extension;
    let config = ImapConfig::new()
        .without(Extension::ListExtended)
        .user(USER, PASSWORD);
    let fake = ImapServer::start(config).await.unwrap();
    fake.user(USER)
        .create_special_folder("Sent Items", FakeSpecialUse::Sent)
        .unwrap();
    let mut session = open(&fake, Security::Tls, &password()).await.unwrap();
    let folders = session.list_folders().await.unwrap();
    assert!(fake.commands().iter().any(|c| c == "LIST \"\" *"));
    let sent = folders.iter().find(|f| f.name == "Sent Items").unwrap();
    assert_eq!(sent.special_use, Some(SpecialUse::Sent));
    session.logout().await.unwrap();
}
