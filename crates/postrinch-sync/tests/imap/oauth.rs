//! Signing in to IMAP with an OAuth access token: XOAUTH2, OAUTHBEARER,
//! and the one retry with a fresh token.

use postrinch_mailtest::{ImapConfig, ImapFault, ImapServer, Mechanism};
use postrinch_model::{Security, SignInReason};
use postrinch_sync::imap::ImapSession;
use postrinch_sync::{SyncError, SyncResult};

use crate::support::{
    Tokens, USER, account, connector, microsoft, oauth_secret, rejected_detail, server, settings,
};

async fn open(fake: &ImapServer, tokens: &Tokens) -> SyncResult<ImapSession> {
    let imap = server(fake.tls_addr().unwrap(), Security::Tls);
    let settings = settings(imap.clone(), imap, microsoft());
    let connector = connector(fake.certificate());
    ImapSession::open(&connector, &account(), &settings, &oauth_secret(), tokens).await
}

fn fake_taking(token: &str) -> ImapConfig {
    ImapConfig::new().oauth_token(USER, token)
}

#[tokio::test]
async fn signs_in_with_xoauth2() {
    let fake = ImapServer::start(fake_taking("good")).await.unwrap();
    let tokens = Tokens::new(&["good"]);
    let session = open(&fake, &tokens).await.unwrap();
    assert_eq!(fake.command_count("AUTHENTICATE"), 1);
    assert!(
        fake.commands()
            .iter()
            .any(|c| c.starts_with("AUTHENTICATE XOAUTH2"))
    );
    assert_eq!((tokens.fetched(), tokens.invalidated()), (1, 0));
    session.logout().await.unwrap();
}

#[tokio::test]
async fn a_refused_token_is_refreshed_once() {
    let fake = ImapServer::start(fake_taking("fresh")).await.unwrap();
    let tokens = Tokens::new(&["stale", "fresh"]);
    let session = open(&fake, &tokens).await.unwrap();
    assert_eq!(fake.command_count("AUTHENTICATE"), 2);
    assert_eq!((tokens.fetched(), tokens.invalidated()), (2, 1));
    session.logout().await.unwrap();
}

#[tokio::test]
async fn refused_twice_needs_sign_in() {
    let fake = ImapServer::start(fake_taking("fresh")).await.unwrap();
    let tokens = Tokens::new(&["stale", "also stale", "fresh"]);
    let detail = rejected_detail(open(&fake, &tokens).await);
    assert_eq!(detail, "Invalid credentials");
    // One retry, no more.
    assert_eq!(fake.command_count("AUTHENTICATE"), 2);
    assert_eq!((tokens.fetched(), tokens.invalidated()), (2, 1));
}

#[tokio::test]
async fn a_refused_refresh_needs_sign_in() {
    let fake = ImapServer::start(fake_taking("fresh")).await.unwrap();
    let tokens = Tokens::new(&["stale"]);
    let detail = rejected_detail(open(&fake, &tokens).await);
    assert_eq!(detail, "invalid_grant");
    assert_eq!(fake.command_count("AUTHENTICATE"), 1);
}

#[tokio::test]
async fn uses_oauthbearer_where_it_is_all_that_is_offered() {
    let config = fake_taking("good").mechanisms(&[Mechanism::OAuthBearer]);
    let fake = ImapServer::start(config).await.unwrap();
    let tokens = Tokens::new(&["stale", "good"]);
    let session = open(&fake, &tokens).await.unwrap();
    let commands = fake.commands();
    let bearer = commands
        .iter()
        .filter(|c| c.starts_with("AUTHENTICATE OAUTHBEARER"))
        .count();
    assert_eq!(bearer, 2, "{commands:?}");
    assert_eq!(tokens.invalidated(), 1);
    session.logout().await.unwrap();
}

#[tokio::test]
async fn an_admins_refusal_is_not_retried() {
    let fake = ImapServer::start(fake_taking("good")).await.unwrap();
    let text = "[CONTACTADMIN] Your organization blocks this app";
    fake.fail_next("AUTHENTICATE", 1, ImapFault::No(text.into()));
    let tokens = Tokens::new(&["good"]);
    match open(&fake, &tokens).await {
        Err(SyncError::NeedsSignIn(SignInReason::AdminPolicy { detail })) => {
            assert_eq!(detail, "Your organization blocks this app");
        }
        other => panic!("expected the admin's refusal, got {other:?}"),
    }
    assert_eq!(fake.command_count("AUTHENTICATE"), 1);
    assert_eq!(tokens.invalidated(), 0);
}

#[tokio::test]
async fn a_server_that_cannot_check_now_is_not_a_sign_in_problem() {
    let fake = ImapServer::start(fake_taking("good")).await.unwrap();
    let text = "[UNAVAILABLE] Authentication backend down";
    fake.fail_next("AUTHENTICATE", 1, ImapFault::No(text.into()));
    let tokens = Tokens::new(&["good"]);
    match open(&fake, &tokens).await {
        Err(SyncError::Server { message, .. }) => {
            assert_eq!(message, "Authentication backend down");
        }
        other => panic!("expected a server failure, got {other:?}"),
    }
    assert_eq!(tokens.invalidated(), 0);
}

/// Answers the first AUTHENTICATE with `NO <text>` and signs in with what
/// comes of it, a good token being on offer every time.
async fn refused_with(text: &str) -> (SyncResult<ImapSession>, Tokens, ImapServer) {
    let fake = ImapServer::start(fake_taking("good")).await.unwrap();
    fake.fail_next("AUTHENTICATE", 1, ImapFault::No(text.into()));
    let tokens = Tokens::new(&["good", "good"]);
    let result = open(&fake, &tokens).await;
    (result, tokens, fake)
}

#[tokio::test]
async fn temporary_refusals_are_the_servers_and_not_retried() {
    for code in [
        "UNAVAILABLE",
        "INUSE",
        "LIMIT",
        "SERVERBUG",
        "CORRUPTION",
        "PRIVACYREQUIRED",
        "NEWFANGLED",
    ] {
        let (result, tokens, fake) = refused_with(&format!("[{code}] Try later")).await;
        match result {
            Err(SyncError::Server { message, .. }) => assert_eq!(message, "Try later", "{code}"),
            other => panic!("{code}: expected a server failure, got {other:?}"),
        }
        assert_eq!(fake.command_count("AUTHENTICATE"), 1, "{code}");
        assert_eq!((tokens.fetched(), tokens.invalidated()), (1, 0), "{code}");
    }
}

#[tokio::test]
async fn credential_refusals_get_one_fresh_token() {
    // The fake signs in the retry, so each of these is retried once.
    for text in [
        "[AUTHENTICATIONFAILED] Invalid credentials",
        "[AUTHORIZATIONFAILED] Not for this user",
        "[EXPIRED] Token expired",
        "[ALERT] Application-specific password required",
        "Invalid credentials",
    ] {
        let (result, tokens, fake) = refused_with(text).await;
        result.unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert_eq!(fake.command_count("AUTHENTICATE"), 2, "{text}");
        assert_eq!((tokens.fetched(), tokens.invalidated()), (2, 1), "{text}");
    }
}

#[tokio::test]
async fn a_credential_refused_twice_keeps_the_servers_words() {
    let fake = ImapServer::start(fake_taking("good")).await.unwrap();
    let text = "[EXPIRED] Token expired";
    fake.fail_next("AUTHENTICATE", 2, ImapFault::No(text.into()));
    let tokens = Tokens::new(&["good", "good"]);
    assert_eq!(rejected_detail(open(&fake, &tokens).await), "Token expired");
    assert_eq!(fake.command_count("AUTHENTICATE"), 2);
}
