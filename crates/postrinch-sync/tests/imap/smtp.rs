//! Signing in to SMTP with an OAuth token: OAUTHBEARER, and a server
//! that keeps challenging after the error challenge's answer.

use postrinch_mailtest::{Mechanism, SmtpConfig, SmtpServer};
use postrinch_model::Security;
use postrinch_sync::SyncResult;
use postrinch_sync::imap::SmtpConnection;

use crate::support::{
    Tokens, USER, account, connector, microsoft, oauth_secret, rejected_detail, server, settings,
};

async fn open(fake: &SmtpServer, tokens: &Tokens) -> SyncResult<SmtpConnection> {
    let smtp = server(fake.tls_addr().unwrap(), Security::Tls);
    let settings = settings(smtp.clone(), smtp, microsoft());
    let connector = connector(fake.certificate());
    SmtpConnection::open(&connector, &account(), &settings, &oauth_secret(), tokens).await
}

#[tokio::test]
async fn signs_in_with_oauthbearer_where_it_is_all_that_is_offered() {
    let config = SmtpConfig::new()
        .oauth_token(USER, "good")
        .mechanisms(&[Mechanism::OAuthBearer]);
    let fake = SmtpServer::start(config).await.unwrap();
    let tokens = Tokens::new(&["stale", "good"]);
    let connection = open(&fake, &tokens).await.unwrap();
    let commands = fake.commands();
    let bearer = commands
        .iter()
        .filter(|c| c.starts_with("AUTH OAUTHBEARER"))
        .count();
    assert_eq!(bearer, 2, "{commands:?}");
    assert_eq!((tokens.fetched(), tokens.invalidated()), (2, 1));
    connection.quit().await.unwrap();
}

#[tokio::test]
async fn a_server_challenging_again_has_refused_the_sign_in() {
    for mechanism in [Mechanism::XOAuth2, Mechanism::OAuthBearer] {
        let config = SmtpConfig::new()
            .oauth_token(USER, "good")
            .mechanisms(&[mechanism])
            .challenges_again(true);
        let fake = SmtpServer::start(config).await.unwrap();
        let tokens = Tokens::new(&["stale", "also stale", "good"]);
        let detail = rejected_detail(open(&fake, &tokens).await);
        // The error challenge's own words, decoded.
        assert!(detail.starts_with('{'), "{mechanism:?}: {detail}");
        assert!(detail.contains("status"), "{mechanism:?}: {detail}");
        // One fresh token, as for any refusal; the connection stayed usable.
        assert_eq!(fake.command_count("AUTH"), 2, "{mechanism:?}");
        assert_eq!(tokens.invalidated(), 1, "{mechanism:?}");
    }
}
