//! Telling an IMAP server's temporary limits from a refused sign-in, with
//! the words Gmail and others really send: a limit is `SyncError::Busy`,
//! never "needs to sign in", and an OAuth account gets no fresh token for
//! it. SMTP's are in `smtp_limits`.

use postrinch_mailtest::{ImapConfig, ImapFault, ImapServer};
use postrinch_model::{ImapSettings, Security, SignInMethod};
use postrinch_sync::imap::{CheckFailure, Connector, ImapSecret, ImapSession, check_account};
use postrinch_sync::{SyncError, SyncResult};

use crate::support::{
    PASSWORD, Tokens, USER, account, busy_message, connector, microsoft, oauth_secret,
    rejected_detail, server, settings,
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

#[tokio::test]
async fn a_busy_server_fails_the_check_as_busy_not_as_sign_in() {
    let (result, _fake) = imap_password(GMAIL_LIMITS[0], false).await;
    let failure = CheckFailure::from(result.unwrap_err());
    assert_eq!(
        failure,
        CheckFailure::Busy("Too many simultaneous connections. (Failure)".into())
    );
}

/// Microsoft 365's answer to the first command after a sign-in it took.
const NOT_CONNECTED: &str = "User is authenticated but not connected.";

/// A fake whose user signs in with a password, and settings and a
/// connector for it over `security`.
async fn password_fake(
    config: ImapConfig,
    security: Security,
) -> (ImapServer, ImapSettings, Connector) {
    let fake = ImapServer::start(config.user(USER, PASSWORD))
        .await
        .unwrap();
    let addr = match security {
        Security::Tls => fake.tls_addr(),
        Security::StartTls => fake.starttls_addr(),
    };
    let imap = server(addr.unwrap(), security);
    let settings = settings(imap.clone(), imap, SignInMethod::Password);
    let connector = connector(fake.certificate());
    (fake, settings, connector)
}

async fn open(settings: &ImapSettings, connector: &Connector) -> SyncResult<ImapSession> {
    let secret = ImapSecret::password(PASSWORD);
    ImapSession::open(connector, &account(), settings, &secret, &Tokens::none()).await
}

#[tokio::test]
async fn a_bad_at_a_limit_after_sign_in_is_busy() {
    // To the CAPABILITY that follows the sign-in.
    let (fake, settings, connector) = password_fake(ImapConfig::new(), Security::Tls).await;
    fake.fail_next("CAPABILITY", 1, ImapFault::Bad(NOT_CONNECTED.into()));
    assert_eq!(
        busy_message(open(&settings, &connector).await, "IMAP"),
        NOT_CONNECTED
    );
    assert_eq!(fake.command_count("AUTHENTICATE"), 1);

    // To the first LIST.
    let mut session = open(&settings, &connector).await.unwrap();
    fake.fail_next("LIST", 1, ImapFault::Bad(NOT_CONNECTED.into()));
    assert_eq!(
        busy_message(session.list_folders().await, "IMAP"),
        NOT_CONNECTED
    );
}

#[tokio::test]
async fn another_bad_after_sign_in_is_the_servers() {
    let (fake, settings, connector) = password_fake(ImapConfig::new(), Security::Tls).await;
    fake.fail_next("CAPABILITY", 1, ImapFault::Bad("Unknown command".into()));
    let result = open(&settings, &connector).await;
    assert!(
        matches!(result, Err(SyncError::Server { .. })),
        "{result:?}"
    );
    // A NO naming a limit is not what Microsoft sends there: not read.
    fake.fail_next("CAPABILITY", 1, ImapFault::No(NOT_CONNECTED.into()));
    let result = open(&settings, &connector).await;
    assert!(
        matches!(result, Err(SyncError::Server { .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn the_check_reports_a_bad_after_sign_in_as_busy() {
    for command in ["CAPABILITY", "LIST"] {
        let (fake, settings, connector) = password_fake(ImapConfig::new(), Security::Tls).await;
        fake.fail_next(command, 1, ImapFault::Bad(NOT_CONNECTED.into()));
        let secret = ImapSecret::password(PASSWORD);
        let check =
            check_account(&connector, &account(), &settings, &secret, &Tokens::none()).await;
        assert_eq!(
            check.imap.unwrap_err(),
            CheckFailure::Busy(NOT_CONNECTED.into()),
            "{command}"
        );
    }
}

#[tokio::test]
async fn a_bye_greeting_at_a_limit_is_busy() {
    let dovecot =
        "Maximum number of connections from user+IP exceeded (mail_max_userip_connections=10)";
    let cases = [
        ("[UNAVAILABLE] Try again later", "Try again later"),
        ("[INUSE] Mailbox in use", "Mailbox in use"),
        (dovecot, dovecot),
    ];
    for security in [Security::Tls, Security::StartTls] {
        for (bye, message) in cases {
            let config = ImapConfig::new().starttls(true).greets_with_bye(bye);
            let (fake, settings, connector) = password_fake(config, security).await;
            let result = open(&settings, &connector).await;
            assert_eq!(busy_message(result, "IMAP"), message, "{security:?}: {bye}");
            assert_eq!(fake.command_count("AUTHENTICATE"), 0, "{bye}");
        }
    }
}

#[tokio::test]
async fn another_bye_greeting_is_the_servers() {
    for bye in [
        "Server shutting down",
        "[SERVERBUG] Too many simultaneous connections",
    ] {
        let config = ImapConfig::new().greets_with_bye(bye);
        let (_fake, settings, connector) = password_fake(config, Security::Tls).await;
        match open(&settings, &connector).await {
            Err(SyncError::Server { message, .. }) => assert!(bye.ends_with(&message), "{bye}"),
            other => panic!("expected a server error, got {other:?}"),
        }
    }
}
