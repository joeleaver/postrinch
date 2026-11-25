//! The IDLE connection held while it is made (`idle.rs`, `idle_link.rs`):
//! a connection of the account refused for its connection limit while the
//! IDLE connection signs in or EXAMINEs the inbox finds it to ask to give
//! way, as the server already counts it. Against the fake server, each
//! step held unanswered in turn.

use std::sync::Arc;
use std::time::Duration;

use postrinch_mailtest::{ImapConfig, ImapFault, ImapServer};
use postrinch_model::{
    Account, EmailAddress, FolderRoles, ImapSettings, Provider, Security, Server, SignInMethod,
    SignInReason,
};
use postrinch_store::Store;

use super::ImapSync;
use crate::imap::{
    AccessTokens, Connector, ImapSecret, ImapSecrets, Timeouts, TlsConfig, TokenFuture,
};
use crate::{MemorySecrets, SyncError, SyncResult};

const USER: &str = "ada@example.org";
const PASSWORD: &str = "correct horse";

/// A password account: no access token is ever asked for.
struct NoTokens;

impl AccessTokens for NoTokens {
    fn access_token<'a>(&'a self, _: &'a EmailAddress) -> TokenFuture<'a, SyncResult<String>> {
        Box::pin(async {
            Err(SyncError::NeedsSignIn(SignInReason::CredentialRejected {
                detail: "a password account has no grant".into(),
            }))
        })
    }

    fn invalidate<'a>(&'a self, _: &'a EmailAddress) -> TokenFuture<'a, ()> {
        Box::pin(async {})
    }
}

struct Fixture {
    server: ImapServer,
    sync: ImapSync,
    me: EmailAddress,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let server = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
        .await
        .unwrap();
    let me = EmailAddress::parse(USER).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let at = |port| Server {
        host: "127.0.0.1".into(),
        port,
        security: Security::Tls,
        username: USER.into(),
    };
    let mut account = Account::new(me.clone(), Provider::Imap);
    account.imap = Some(ImapSettings {
        imap: at(server.tls_addr().unwrap().port()),
        // Never used here.
        smtp: at(1),
        sign_in: SignInMethod::Password,
        roles: FolderRoles::default(),
        server_files_sent: true,
        server_files_sent_chosen: false,
    });
    store.put_account(&account).unwrap();
    let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
    secrets
        .set(&me, &ImapSecret::password(PASSWORD))
        .await
        .unwrap();
    let tls = TlsConfig::with_extra_roots(vec![server.certificate().der().clone()]).unwrap();
    let connector = Connector::new(tls).with_timeouts(Timeouts {
        connect: Duration::from_secs(5),
        command: Duration::from_secs(5),
    });
    let sync = ImapSync::new(store, connector, secrets, Arc::new(NoTokens));
    Fixture {
        server,
        sync,
        me,
        _dir: dir,
    }
}

impl Fixture {
    /// Waits (up to 10 s) until the server has had `n` commands named
    /// `command`.
    async fn until_seen(&self, command: &str, n: usize) {
        let seen = tokio::time::timeout(Duration::from_secs(10), async {
            while self.server.command_count(command) < n {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(seen.is_ok(), "no {command} came");
    }

    /// Asks the IDLE connection to give way, as a connection refused for
    /// the account's connection limit does, waiting for it not at all.
    /// Returns whether it was asked.
    async fn ask_to_give_way(&self) -> bool {
        let mut asked = self.sync.give_way_asked(&self.me);
        asked.mark_unchanged();
        let gone = self.sync.links.give_way(&self.me, Duration::ZERO).await;
        assert!(!gone, "not gone while it is held");
        asked.has_changed().unwrap()
    }
}

#[tokio::test]
async fn the_idle_connection_is_held_from_before_it_signs_in_until_it_goes() {
    let f = fixture().await;
    let sign_in = f.server.hold_next("AUTHENTICATE", "");
    let examine = f.server.hold_next("EXAMINE", "\"INBOX\"");
    let opening = f.sync.idle_connection(&f.me);
    let asking = async {
        // Its sign-in sent, unanswered: the server may count it already.
        f.until_seen("AUTHENTICATE", 1).await;
        assert!(f.ask_to_give_way().await, "asked while signing in");
        sign_in.let_go();
        // Signed in, its EXAMINE unanswered.
        f.until_seen("EXAMINE", 1).await;
        assert!(f.ask_to_give_way().await, "asked while EXAMINing");
        examine.let_go();
    };
    let (opened, ()) = tokio::join!(opening, asking);
    let idler = opened.unwrap().expect("the fake offers IDLE");
    assert!(f.ask_to_give_way().await, "asked once made");
    idler.logout().await;
    let mut asked = f.sync.give_way_asked(&f.me);
    asked.mark_unchanged();
    assert!(!f.sync.links.give_way(&f.me, Duration::ZERO).await);
    assert!(!asked.has_changed().unwrap(), "nothing left to ask");
}

#[tokio::test]
async fn an_idle_connection_that_cannot_watch_the_inbox_is_held_no_longer() {
    let f = fixture().await;
    f.server.fail_next(
        "EXAMINE",
        1,
        ImapFault::No("[UNAVAILABLE] Try later".into()),
    );
    let refused = f.sync.idle_connection(&f.me).await;
    assert!(
        matches!(refused, Err(SyncError::Server { .. })),
        "{refused:?}"
    );
    let mut asked = f.sync.give_way_asked(&f.me);
    asked.mark_unchanged();
    assert!(!f.sync.links.give_way(&f.me, Duration::ZERO).await);
    assert!(!asked.has_changed().unwrap(), "nothing left to ask");
}
