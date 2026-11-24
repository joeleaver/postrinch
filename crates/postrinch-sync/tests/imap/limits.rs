//! Telling a server's temporary limits from a refused sign-in, with the
//! words Gmail and others really send: a limit is `SyncError::Busy`, never
//! "needs to sign in", and an OAuth account gets no fresh token for it.

use postrinch_mailtest::{
    ImapConfig, ImapFault, ImapServer, Mechanism, SmtpConfig, SmtpFault, SmtpServer, SmtpStage,
};
use postrinch_model::{Security, SignInMethod};
use postrinch_sync::imap::{CheckFailure, ImapSecret, ImapSession, SmtpConnection};
use postrinch_sync::{SyncError, SyncResult};

use crate::support::{
    PASSWORD, Tokens, USER, account, connector, microsoft, oauth_secret, rejected_detail, server,
    settings,
};

/// Gmail's temporary limits, as its IMAP server answers a sign-in.
const GMAIL_LIMITS: [&str; 3] = [
    "[ALERT] Too many simultaneous connections. (Failure)",
    "[ALERT] Account exceeded command or bandwidth limits. (Failure)",
    "[UNAVAILABLE] Temporary System Error",
];

/// Gmail's real refusals, and the words each keeps.
const GMAIL_REFUSALS: [(&str, &str); 5] = [
    (
        "[ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)",
        "Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)",
    ),
    (
        "[ALERT] Please log in via your web browser: https://support.google.com/mail/accounts/answer/78754 (Failure)",
        "Please log in via your web browser: https://support.google.com/mail/accounts/answer/78754 (Failure)",
    ),
    (
        "[ALERT] Web login required: https://support.google.com/mail/answer/78754 (Failure)",
        "Web login required: https://support.google.com/mail/answer/78754 (Failure)",
    ),
    (
        "[ALERT] Your account is not enabled for IMAP use. (Failure)",
        "Your account is not enabled for IMAP use. (Failure)",
    ),
    (
        "[AUTHENTICATIONFAILED] Invalid credentials (Failure)",
        "Invalid credentials (Failure)",
    ),
];

/// The words of a busy server, or a panic saying what came.
fn busy_message<T: std::fmt::Debug>(result: SyncResult<T>, service: &str) -> String {
    match result {
        Err(SyncError::Busy {
            service: s,
            message,
            ..
        }) if s == service => message,
        other => panic!("expected a busy {service} server, got {other:?}"),
    }
}

/// The text after a `[CODE]`, as the person is shown it.
fn words(answer: &str) -> &str {
    answer.split_once("] ").map_or(answer, |(_, text)| text)
}

/// Signs in to IMAP with a password, the first sign-in answered `NO
/// <answer>`: over AUTHENTICATE PLAIN, or over LOGIN when no mechanism is
/// offered.
async fn imap_password(answer: &str, login: bool) -> (SyncResult<ImapSession>, ImapServer) {
    let mut config = ImapConfig::new().user(USER, PASSWORD);
    if login {
        config = config.mechanisms(&[]);
    }
    let fake = ImapServer::start(config).await.unwrap();
    let command = if login { "LOGIN" } else { "AUTHENTICATE" };
    fake.fail_next(command, 1, ImapFault::No(answer.into()));
    let imap = server(fake.tls_addr().unwrap(), Security::Tls);
    let settings = settings(imap.clone(), imap, SignInMethod::Password);
    let connector = connector(fake.certificate());
    let secret = ImapSecret::password(PASSWORD);
    let result =
        ImapSession::open(&connector, &account(), &settings, &secret, &Tokens::none()).await;
    (result, fake)
}

/// Signs in to IMAP with OAuth, the first `times` AUTHENTICATEs answered
/// with `fault`, a good token on offer every time.
async fn imap_oauth(
    times: usize,
    fault: ImapFault,
) -> (SyncResult<ImapSession>, Tokens, ImapServer) {
    let fake = ImapServer::start(ImapConfig::new().oauth_token(USER, "good"))
        .await
        .unwrap();
    fake.fail_next("AUTHENTICATE", times, fault);
    let imap = server(fake.tls_addr().unwrap(), Security::Tls);
    let settings = settings(imap.clone(), imap, microsoft());
    let connector = connector(fake.certificate());
    let tokens = Tokens::new(&["good", "good", "good"]);
    let result =
        ImapSession::open(&connector, &account(), &settings, &oauth_secret(), &tokens).await;
    (result, tokens, fake)
}

#[tokio::test]
async fn gmails_limits_are_busy_over_authenticate_and_login() {
    for login in [false, true] {
        for answer in GMAIL_LIMITS {
            let (result, fake) = imap_password(answer, login).await;
            assert_eq!(busy_message(result, "IMAP"), words(answer), "{answer}");
            // Asked once: a limit is not argued with.
            let sign_ins = fake.command_count("AUTHENTICATE") + fake.command_count("LOGIN");
            assert_eq!(sign_ins, 1, "{answer}");
        }
    }
}

#[tokio::test]
async fn gmails_refusals_still_need_sign_in_over_authenticate_and_login() {
    for login in [false, true] {
        for (answer, detail) in GMAIL_REFUSALS {
            let (result, _fake) = imap_password(answer, login).await;
            assert_eq!(rejected_detail(result), detail, "login {login}: {answer}");
        }
    }
}

#[tokio::test]
async fn other_servers_limits_are_busy() {
    for answer in [
        "[INUSE] Mailbox in use",
        "[LIMIT] Too many connections",
        "Maximum number of connections from user+IP exceeded (mail_max_userip_connections=10)",
        "Temporary authentication failure.",
        "User is authenticated but not connected.",
    ] {
        let (result, _fake) = imap_password(answer, false).await;
        assert_eq!(busy_message(result, "IMAP"), words(answer), "{answer}");
    }
}

#[tokio::test]
async fn microsofts_bad_while_shedding_sessions_is_busy() {
    let fault = ImapFault::Bad("User is authenticated but not connected.".into());
    let (result, tokens, fake) = imap_oauth(1, fault).await;
    assert_eq!(
        busy_message(result, "IMAP"),
        "User is authenticated but not connected."
    );
    assert_eq!(fake.command_count("AUTHENTICATE"), 1);
    assert_eq!((tokens.fetched(), tokens.invalidated()), (1, 0));
}

#[tokio::test]
async fn an_oauth_account_at_a_limit_gets_no_fresh_token() {
    for answer in GMAIL_LIMITS {
        let (result, tokens, fake) = imap_oauth(1, ImapFault::No(answer.into())).await;
        assert_eq!(busy_message(result, "IMAP"), words(answer), "{answer}");
        assert_eq!(fake.command_count("AUTHENTICATE"), 1, "{answer}");
        assert_eq!((tokens.fetched(), tokens.invalidated()), (1, 0), "{answer}");
    }
}

#[tokio::test]
async fn an_oauth_account_refused_gets_one_fresh_token_then_needs_sign_in() {
    for (answer, detail) in GMAIL_REFUSALS {
        let (result, tokens, fake) = imap_oauth(2, ImapFault::No(answer.into())).await;
        assert_eq!(rejected_detail(result), detail, "{answer}");
        assert_eq!(fake.command_count("AUTHENTICATE"), 2, "{answer}");
        assert_eq!((tokens.fetched(), tokens.invalidated()), (2, 1), "{answer}");
    }
}

/// Signs in to SMTP, the first AUTH answered `code text`: with a password
/// (PLAIN), or with OAuth (XOAUTH2) and a good token every time.
async fn smtp(
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
    let fault = SmtpFault::Reply {
        code,
        text: text.into(),
    };
    fake.fail_next(SmtpStage::Auth, 1, fault);
    let smtp = server(fake.tls_addr().unwrap(), Security::Tls);
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
async fn a_busy_server_fails_the_check_as_busy_not_as_sign_in() {
    let (result, _fake) = imap_password(GMAIL_LIMITS[0], false).await;
    let failure = CheckFailure::from(result.unwrap_err());
    assert_eq!(
        failure,
        CheckFailure::Busy("Too many simultaneous connections. (Failure)".into())
    );
}
