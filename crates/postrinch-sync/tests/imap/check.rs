//! The add-account check: IMAP sign-in and folder list, SMTP sign-in,
//! each server's outcome its own.

use postrinch_mailtest::{
    ImapConfig, ImapFault, ImapServer, SmtpConfig, SmtpServer, SpecialUse as FakeSpecialUse,
};
use postrinch_model::{Security, Server, SignInMethod, SignInReason};
use postrinch_sync::imap::{
    AccessTokens, AccountCheck, CheckFailure, Connector, ImapSecret, SpecialUse, TlsConfig,
    check_account,
};

use crate::support::{
    PASSWORD, Tokens, USER, account, closed_port, microsoft, oauth_secret, server, settings,
};

/// Both fakes, sharing one certificate so one connector trusts both.
pub struct Fakes {
    pub imap: ImapServer,
    pub smtp: SmtpServer,
}

impl Fakes {
    async fn start(imap: ImapConfig, smtp: SmtpConfig) -> Self {
        let imap = ImapServer::start(imap).await.unwrap();
        let smtp = SmtpServer::start(smtp).await.unwrap();
        imap.user(USER)
            .create_special_folder("Sent", FakeSpecialUse::Sent)
            .unwrap();
        Self { imap, smtp }
    }

    pub async fn with_passwords() -> Self {
        Self::start(
            ImapConfig::new().user(USER, PASSWORD),
            SmtpConfig::new().user(USER, PASSWORD),
        )
        .await
    }

    pub fn imap_server(&self) -> Server {
        server(self.imap.tls_addr().unwrap(), Security::Tls)
    }

    pub fn smtp_server(&self, security: Security) -> Server {
        let addr = match security {
            Security::Tls => self.smtp.tls_addr(),
            Security::StartTls => self.smtp.starttls_addr(),
        };
        server(addr.unwrap(), security)
    }

    /// Trusts both fakes' certificates.
    pub fn connector(&self) -> Connector {
        let roots = vec![
            self.imap.certificate().der().clone(),
            self.smtp.certificate().der().clone(),
        ];
        Connector::new(TlsConfig::with_extra_roots(roots).unwrap())
    }

    async fn check(
        &self,
        imap: Server,
        smtp: Server,
        sign_in: SignInMethod,
        secret: &ImapSecret,
        tokens: &dyn AccessTokens,
    ) -> AccountCheck {
        let settings = settings(imap, smtp, sign_in);
        check_account(&self.connector(), &account(), &settings, secret, tokens).await
    }

    async fn check_password(&self, smtp: Server, secret: &ImapSecret) -> AccountCheck {
        let method = match secret {
            ImapSecret::Password {
                smtp_password: Some(_),
                ..
            } => SignInMethod::SeparateSmtpPassword,
            _ => SignInMethod::Password,
        };
        self.check(self.imap_server(), smtp, method, secret, &Tokens::none())
            .await
    }
}

fn password() -> ImapSecret {
    ImapSecret::password(PASSWORD)
}

fn assert_sent_folder(check: &AccountCheck) {
    let folders = check.imap.as_ref().unwrap();
    let sent = folders.iter().find(|f| f.name == "Sent").unwrap();
    assert_eq!(sent.special_use, Some(SpecialUse::Sent));
}

#[tokio::test]
async fn passes_with_smtp_over_tls_and_over_starttls() {
    let fakes = Fakes::with_passwords().await;
    for security in [Security::Tls, Security::StartTls] {
        let check = fakes
            .check_password(fakes.smtp_server(security), &password())
            .await;
        assert!(check.is_ok(), "{security:?}: {check:?}");
        assert_sent_folder(&check);
    }
    assert_eq!(fakes.smtp.command_count("STARTTLS"), 1);
    // The check sends no mail.
    assert!(fakes.smtp.messages().is_empty());
    assert_eq!(fakes.smtp.command_count("MAIL"), 0);
}

#[tokio::test]
async fn a_server_dropping_the_connection_at_logout_still_passes() {
    let fakes = Fakes::with_passwords().await;
    fakes
        .imap
        .fail_next("LOGOUT", 1, ImapFault::DisconnectBefore);
    let check = fakes
        .check_password(fakes.smtp_server(Security::Tls), &password())
        .await;
    assert!(check.is_ok(), "{check:?}");
    assert_sent_folder(&check);
    assert_eq!(fakes.imap.command_count("LOGOUT"), 1);
}

#[tokio::test]
async fn says_imap_is_down_and_smtp_still_passes() {
    let fakes = Fakes::with_passwords().await;
    let imap = server(closed_port().await, Security::Tls);
    let settings = settings(
        imap,
        fakes.smtp_server(Security::Tls),
        SignInMethod::Password,
    );
    let check = check_account(
        &fakes.connector(),
        &account(),
        &settings,
        &password(),
        &Tokens::none(),
    )
    .await;
    assert!(
        matches!(check.imap, Err(CheckFailure::Unreachable(_))),
        "{check:?}"
    );
    assert_eq!(check.smtp, Ok(()));
}

#[tokio::test]
async fn says_smtp_refused_the_password_in_its_words() {
    let fakes = Fakes::start(
        ImapConfig::new().user(USER, PASSWORD),
        SmtpConfig::new().user(USER, "another"),
    )
    .await;
    let check = fakes
        .check_password(fakes.smtp_server(Security::Tls), &password())
        .await;
    assert_sent_folder(&check);
    assert_eq!(
        check.smtp,
        Err(CheckFailure::SignIn(SignInReason::CredentialRejected {
            detail: "Authentication credentials invalid".into(),
        }))
    );
}

#[tokio::test]
async fn a_separate_smtp_password_signs_in_to_smtp() {
    let fakes = Fakes::start(
        ImapConfig::new().user(USER, PASSWORD),
        SmtpConfig::new().user(USER, "sending"),
    )
    .await;
    let secret = ImapSecret::Password {
        password: PASSWORD.into(),
        smtp_password: Some("sending".into()),
    };
    let check = fakes
        .check_password(fakes.smtp_server(Security::StartTls), &secret)
        .await;
    assert!(check.is_ok(), "{check:?}");
}

#[tokio::test]
async fn smtp_without_starttls_gets_no_credentials() {
    let fakes = Fakes::start(
        ImapConfig::new().user(USER, PASSWORD),
        SmtpConfig::new().user(USER, PASSWORD).offer_starttls(false),
    )
    .await;
    let check = fakes
        .check_password(fakes.smtp_server(Security::StartTls), &password())
        .await;
    assert!(check.imap.is_ok(), "{check:?}");
    assert_eq!(check.smtp, Err(CheckFailure::Insecure));
    assert_eq!(fakes.smtp.command_count("AUTH"), 0);
}

/// Checks an OAuth account's SMTP alone (IMAP points at a closed port, so
/// only SMTP asks for tokens).
async fn check_smtp_oauth(fakes: &Fakes, security: Security, tokens: &Tokens) -> AccountCheck {
    let imap = server(closed_port().await, Security::Tls);
    let smtp = fakes.smtp_server(security);
    fakes
        .check(imap, smtp, microsoft(), &oauth_secret(), tokens)
        .await
}

async fn oauth_fakes() -> Fakes {
    Fakes::start(
        ImapConfig::new().oauth_token(USER, "fresh"),
        SmtpConfig::new().oauth_token(USER, "fresh"),
    )
    .await
}

#[tokio::test]
async fn oauth_passes_on_both_servers() {
    let fakes = oauth_fakes().await;
    let tokens = Tokens::new(&["fresh"]);
    let check = fakes
        .check(
            fakes.imap_server(),
            fakes.smtp_server(Security::StartTls),
            microsoft(),
            &oauth_secret(),
            &tokens,
        )
        .await;
    assert!(check.is_ok(), "{check:?}");
    assert_eq!(tokens.invalidated(), 0);
}

#[tokio::test]
async fn smtp_refreshes_a_refused_token_once() {
    let fakes = oauth_fakes().await;
    let tokens = Tokens::new(&["stale", "fresh"]);
    let check = check_smtp_oauth(&fakes, Security::Tls, &tokens).await;
    assert_eq!(check.smtp, Ok(()));
    assert_eq!(fakes.smtp.command_count("AUTH"), 2);
    assert_eq!((tokens.fetched(), tokens.invalidated()), (2, 1));
}

#[tokio::test]
async fn smtp_refusing_a_token_twice_needs_sign_in() {
    let fakes = oauth_fakes().await;
    let tokens = Tokens::new(&["stale", "also stale", "fresh"]);
    let check = check_smtp_oauth(&fakes, Security::Tls, &tokens).await;
    assert_eq!(
        check.smtp,
        Err(CheckFailure::SignIn(SignInReason::CredentialRejected {
            detail: "Authentication credentials invalid".into(),
        }))
    );
    assert_eq!(fakes.smtp.command_count("AUTH"), 2);
    assert_eq!(tokens.invalidated(), 1);
}
