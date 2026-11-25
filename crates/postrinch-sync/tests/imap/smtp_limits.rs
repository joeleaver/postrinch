//! An SMTP server's "later" (any 4xx, at the greeting, EHLO, STARTTLS or
//! AUTH) is `SyncError::Busy`, never "needs to sign in", and an OAuth
//! account gets no fresh token for it; 534, 535 and AUTH's 432 are the
//! person's to act on.

use postrinch_mailtest::{
    ImapConfig, ImapServer, Mechanism, SmtpConfig, SmtpFault, SmtpServer, SmtpStage,
};
use postrinch_model::{Security, SignInMethod};
use postrinch_sync::SyncResult;
use postrinch_sync::imap::{
    CheckFailure, Connector, ImapSecret, SmtpConnection, TlsConfig, check_account,
};

use crate::support::{
    PASSWORD, Tokens, USER, account, busy_message, connector, microsoft, oauth_secret,
    rejected_detail, server, settings,
};

fn reply(code: u16, text: &str) -> SmtpFault {
    SmtpFault::Reply {
        code,
        text: text.into(),
    }
}

/// Signs in to SMTP over `security`, the first time `stage` is reached
/// answered `code text`: with a password (PLAIN), or with OAuth (XOAUTH2)
/// and a good token every time.
async fn smtp_at(
    stage: SmtpStage,
    security: Security,
    code: u16,
    text: &str,
    oauth: bool,
) -> (SyncResult<SmtpConnection>, Tokens, SmtpServer) {
    let config = if oauth {
        SmtpConfig::new()
            .oauth_token(USER, "good")
            .mechanisms(&[Mechanism::XOAuth2])
    } else {
        SmtpConfig::new().user(USER, PASSWORD)
    };
    let fake = SmtpServer::start(config).await.unwrap();
    fake.fail_next(stage, 1, reply(code, text));
    let addr = match security {
        Security::Tls => fake.tls_addr(),
        Security::StartTls => fake.starttls_addr(),
    };
    let smtp = server(addr.unwrap(), security);
    let connector = connector(fake.certificate());
    let tokens = Tokens::new(&["good", "good"]);
    let result = if oauth {
        let settings = settings(smtp.clone(), smtp, microsoft());
        SmtpConnection::open(&connector, &account(), &settings, &oauth_secret(), &tokens).await
    } else {
        let settings = settings(smtp.clone(), smtp, SignInMethod::Password);
        let secret = ImapSecret::password(PASSWORD);
        SmtpConnection::open(&connector, &account(), &settings, &secret, &tokens).await
    };
    (result, tokens, fake)
}

/// As [`smtp_at`], at AUTH over implicit TLS.
async fn smtp(
    code: u16,
    text: &str,
    oauth: bool,
) -> (SyncResult<SmtpConnection>, Tokens, SmtpServer) {
    smtp_at(SmtpStage::Auth, Security::Tls, code, text, oauth).await
}

/// Gmail's SMTP answers that mean "later".
const SMTP_LIMITS: [(u16, &str, &str); 3] = [
    (
        421,
        "4.7.0 Try again later, closing connection. (AUTH)",
        "Try again later, closing connection. (AUTH)",
    ),
    (
        454,
        "4.7.0 Too many login attempts, please try again later.",
        "Too many login attempts, please try again later.",
    ),
    (
        454,
        "4.7.0 Cannot sign in due to a temporary system problem. Try again later.",
        "Cannot sign in due to a temporary system problem. Try again later.",
    ),
];

/// "Later" before AUTH: the stage, the port it is reached on, the reply
/// and the words kept.
const BEFORE_AUTH: [(SmtpStage, Security, u16, &str, &str); 4] = [
    (
        SmtpStage::Greeting,
        Security::Tls,
        421,
        "4.3.2 Service not available, closing transmission channel",
        "Service not available, closing transmission channel",
    ),
    (
        SmtpStage::Ehlo,
        Security::Tls,
        451,
        "4.3.0 Temporary server error, try again later",
        "Temporary server error, try again later",
    ),
    (
        SmtpStage::Ehlo,
        Security::StartTls,
        421,
        "4.7.0 Try again later, closing connection. (EHLO)",
        "Try again later, closing connection. (EHLO)",
    ),
    (
        SmtpStage::StartTls,
        Security::StartTls,
        454,
        "4.7.0 TLS not available due to temporary reason",
        "TLS not available due to temporary reason",
    ),
];

#[tokio::test]
async fn smtp_4xx_at_sign_in_is_busy_and_gets_no_fresh_token() {
    for oauth in [false, true] {
        for (code, text, message) in SMTP_LIMITS {
            let (result, tokens, fake) = smtp(code, text, oauth).await;
            assert_eq!(busy_message(result, "SMTP"), message, "{code} {text}");
            assert_eq!(fake.command_count("AUTH"), 1, "oauth {oauth}: {code}");
            assert_eq!(tokens.invalidated(), 0, "oauth {oauth}: {code}");
        }
    }
}

#[tokio::test]
async fn smtp_4xx_before_sign_in_is_busy_and_sends_no_credential() {
    for oauth in [false, true] {
        for (stage, security, code, text, message) in BEFORE_AUTH {
            let (result, tokens, fake) = smtp_at(stage, security, code, text, oauth).await;
            assert_eq!(busy_message(result, "SMTP"), message, "{stage:?} {code}");
            assert_eq!(fake.command_count("AUTH"), 0, "{stage:?} {code}");
            assert_eq!(tokens.invalidated(), 0, "{stage:?} {code}");
        }
    }
}

#[tokio::test]
async fn a_password_transition_needs_the_person() {
    let text = "4.7.12 A password transition is needed";
    let (result, tokens, fake) = smtp(432, text, false).await;
    assert_eq!(rejected_detail(result), "A password transition is needed");
    assert_eq!(fake.command_count("AUTH"), 1);
    assert_eq!(tokens.fetched(), 0);
}

#[tokio::test]
async fn smtp_534_and_535_still_need_sign_in() {
    let refusals = [
        (
            535,
            "5.7.8 Username and Password not accepted.",
            "Username and Password not accepted.",
        ),
        (
            534,
            "5.7.9 Application-specific password required.",
            "Application-specific password required.",
        ),
    ];
    for (code, text, detail) in refusals {
        let (result, tokens, fake) = smtp(code, text, false).await;
        assert_eq!(rejected_detail(result), detail, "{code}");
        assert_eq!(fake.command_count("AUTH"), 1, "{code}");
        assert_eq!(tokens.fetched(), 0, "{code}");
    }
    // With OAuth, a 535 gets the one fresh token, which the fake takes.
    let (result, tokens, fake) = smtp(535, refusals[0].1, true).await;
    result.unwrap().quit().await.unwrap();
    assert_eq!(fake.command_count("AUTH"), 2);
    assert_eq!((tokens.fetched(), tokens.invalidated()), (2, 1));
}

#[tokio::test]
async fn the_check_reports_smtp_busy_at_every_stage() {
    let stages = SMTP_LIMITS
        .iter()
        .map(|&(code, text, message)| (SmtpStage::Auth, Security::Tls, code, text, message))
        .chain(BEFORE_AUTH);
    for (stage, security, code, text, message) in stages {
        let imap = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
            .await
            .unwrap();
        let smtp = SmtpServer::start(SmtpConfig::new().user(USER, PASSWORD))
            .await
            .unwrap();
        smtp.fail_next(stage, 1, reply(code, text));
        let addr = match security {
            Security::Tls => smtp.tls_addr(),
            Security::StartTls => smtp.starttls_addr(),
        };
        let roots = vec![
            imap.certificate().der().clone(),
            smtp.certificate().der().clone(),
        ];
        let connector = Connector::new(TlsConfig::with_extra_roots(roots).unwrap());
        let settings = settings(
            server(imap.tls_addr().unwrap(), Security::Tls),
            server(addr.unwrap(), security),
            SignInMethod::Password,
        );
        let secret = ImapSecret::password(PASSWORD);
        let check =
            check_account(&connector, &account(), &settings, &secret, &Tokens::none()).await;
        assert!(check.imap.is_ok(), "{stage:?}: {:?}", check.imap);
        assert_eq!(
            check.smtp,
            Err(CheckFailure::Busy(message.into())),
            "{stage:?} {code}"
        );
    }
}
