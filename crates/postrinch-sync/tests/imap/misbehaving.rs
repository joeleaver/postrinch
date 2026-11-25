//! Servers that misbehave: one that never greets, one whose STARTTLS
//! answer comes with plain text smuggled after it, a token provider that
//! never answers. Each ends in an error, never a hang or a sign-in.

use std::time::{Duration, Instant};

use postrinch_mailtest::{ImapConfig, ImapServer, TestCertificate};
use postrinch_model::{EmailAddress, Security, SignInMethod};
use postrinch_sync::imap::{
    AccessTokens, Connector, ImapSecret, ImapSession, Timeouts, TlsConfig, TokenFuture,
};
use postrinch_sync::{SyncError, SyncResult};

use crate::support::{PASSWORD, Tokens, USER, account, microsoft, oauth_secret, server, settings};

/// A connector trusting the fake, with timeouts short enough to wait out.
fn impatient(certificate: &TestCertificate) -> Connector {
    let tls = TlsConfig::with_extra_roots(vec![certificate.der().clone()]).unwrap();
    Connector::new(tls).with_timeouts(Timeouts {
        connect: Duration::from_millis(500),
        command: Duration::from_millis(500),
    })
}

async fn open(
    fake: &ImapServer,
    security: Security,
    sign_in: SignInMethod,
    secret: &ImapSecret,
    tokens: &dyn AccessTokens,
) -> SyncResult<ImapSession> {
    let addr = match security {
        Security::Tls => fake.tls_addr().unwrap(),
        Security::StartTls => fake.starttls_addr().unwrap(),
    };
    let imap = server(addr, security);
    let settings = settings(imap.clone(), imap, sign_in);
    let connector = impatient(fake.certificate());
    ImapSession::open(&connector, &account(), &settings, secret, tokens).await
}

/// The detail of a timeout, or a panic saying what came instead.
fn timed_out<T: std::fmt::Debug>(result: SyncResult<T>) -> String {
    match result {
        Err(SyncError::Unreachable { detail, .. }) => detail,
        other => panic!("expected a timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn a_server_that_never_greets_times_out() {
    let config = ImapConfig::new()
        .starttls(true)
        .greets(false)
        .user(USER, PASSWORD);
    let fake = ImapServer::start(config).await.unwrap();
    let secret = ImapSecret::password(PASSWORD);
    for (security, during) in [
        // Over TLS the handshake is done before the greeting is awaited.
        (Security::Tls, "reading the greeting"),
        // With STARTTLS the plain-text greeting is part of connecting.
        (Security::StartTls, "connecting"),
    ] {
        let started = Instant::now();
        let result = open(
            &fake,
            security,
            SignInMethod::Password,
            &secret,
            &Tokens::none(),
        )
        .await;
        let detail = timed_out(result);
        assert!(detail.contains(during), "{security:?}: {detail}");
        assert!(started.elapsed() < Duration::from_secs(5), "{security:?}");
    }
    assert!(fake.commands().is_empty(), "{:?}", fake.commands());
}

#[tokio::test]
async fn plain_text_after_starttls_is_refused_before_signing_in() {
    let config = ImapConfig::new()
        .implicit_tls(false)
        .starttls(true)
        .injects_after_starttls(true)
        .user(USER, PASSWORD);
    let fake = ImapServer::start(config).await.unwrap();
    let secret = ImapSecret::password(PASSWORD);
    let result = open(
        &fake,
        Security::StartTls,
        SignInMethod::Password,
        &secret,
        &Tokens::none(),
    )
    .await;
    match result {
        Err(SyncError::Server { message, .. }) => {
            assert_eq!(message, "the server sent more than it was asked for");
        }
        other => panic!("expected the injection refused, got {other:?}"),
    }
    assert_eq!(fake.command_count("STARTTLS"), 1);
    assert_eq!(fake.command_count("LOGIN"), 0);
    assert_eq!(fake.command_count("AUTHENTICATE"), 0);
}

/// A token provider that never answers.
struct NeverAnswers;

impl AccessTokens for NeverAnswers {
    fn access_token<'a>(&'a self, _: &'a EmailAddress) -> TokenFuture<'a, SyncResult<String>> {
        Box::pin(std::future::pending())
    }

    fn invalidate<'a>(&'a self, _: &'a EmailAddress) -> TokenFuture<'a, ()> {
        Box::pin(async {})
    }
}

#[tokio::test]
async fn a_token_provider_that_never_answers_times_out() {
    let fake = ImapServer::start(ImapConfig::new().oauth_token(USER, "good"))
        .await
        .unwrap();
    let result = open(
        &fake,
        Security::Tls,
        microsoft(),
        &oauth_secret(),
        &NeverAnswers,
    )
    .await;
    let detail = timed_out(result);
    assert!(detail.contains("getting an access token"), "{detail}");
    assert_eq!(fake.command_count("AUTHENTICATE"), 0);
}
