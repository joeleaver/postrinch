//! `add-imap` against the fake servers of `postrinch-mailtest`, with the
//! secrets in memory: nothing touches the keychain or a real server.

use std::sync::Arc;

use clap::Parser;
use postrinch_discover::{Candidate, Discovery, Username};
use postrinch_mailtest::{ImapConfig, ImapServer, SmtpConfig, SmtpServer, SpecialUse};
use postrinch_model::{EmailAddress, Provider, SignInMethod};
use postrinch_store::Store;
use postrinch_sync::MemorySecrets;
use postrinch_sync::imap::{
    Connector, ImapOAuth, ImapSecret, ImapSecrets, ImapSync, OAuthClients, TlsConfig,
};

use super::*;
use crate::{Cli, Command};

const USER: &str = "ada@example.org";
const PASSWORD: &str = "correct horse";

/// Passwords given in order, as a person typing them would.
struct Given(Vec<&'static str>);

impl Passwords for Given {
    fn password(&mut self, _: &str) -> Result<String> {
        Ok(self.0.remove(0).to_owned())
    }
}

struct Fixture {
    imap: ImapServer,
    smtp: SmtpServer,
    store: Store,
    secrets: ImapSecrets,
    sync: ImapSync,
    oauth: Arc<ImapOAuth>,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let imap = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
        .await
        .unwrap();
    imap.user(USER)
        .create_special_folder("Sent", SpecialUse::Sent)
        .unwrap();
    let smtp = SmtpServer::start(SmtpConfig::new().user(USER, PASSWORD))
        .await
        .unwrap();
    let roots = vec![
        imap.certificate().der().clone(),
        smtp.certificate().der().clone(),
    ];
    let connector = Connector::new(TlsConfig::with_extra_roots(roots).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
    // As the app has it: the OAuth sign-ins give the access tokens.
    let oauth = Arc::new(
        ImapOAuth::new(
            OAuthClients::none(),
            secrets.clone(),
            Arc::new(MemorySecrets::default()),
        )
        .unwrap(),
    );
    let sync = ImapSync::new(store.clone(), connector, secrets.clone(), oauth.clone());
    Fixture {
        imap,
        smtp,
        store,
        secrets,
        sync,
        oauth,
        _dir: dir,
    }
}

/// `add-imap` with these words after it.
fn args(words: &[&str]) -> AddImapArgs {
    let line = ["postrinch-cli", "add-imap"].iter().chain(words);
    match Cli::try_parse_from(line).unwrap().command {
        Command::AddImap(args) => args,
        _ => unreachable!(),
    }
}

impl Fixture {
    /// Options naming both fakes, with TLS.
    fn servers(&self) -> Vec<String> {
        let imap = self.imap.tls_addr().unwrap();
        let smtp = self.smtp.tls_addr().unwrap();
        vec![
            USER.into(),
            "--imap-host".into(),
            imap.ip().to_string(),
            "--imap-port".into(),
            imap.port().to_string(),
            "--smtp-host".into(),
            smtp.ip().to_string(),
            "--smtp-port".into(),
            smtp.port().to_string(),
            "--name".into(),
            "Ada".into(),
        ]
    }

    async fn add(&self, passwords: Vec<&'static str>) -> Result<()> {
        let words = self.servers();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        add_imap(&self.sync, &self.oauth, args(&words), &mut Given(passwords)).await
    }
}

fn ada() -> EmailAddress {
    EmailAddress::parse(USER).unwrap()
}

#[tokio::test]
async fn adds_an_account_both_servers_take() {
    let f = fixture().await;
    f.add(vec![PASSWORD]).await.unwrap();
    let stored = f.store.account(&ada()).unwrap().unwrap();
    assert_eq!(stored.provider, Provider::Imap);
    assert!(stored.is_active());
    assert_eq!(stored.display_name.as_deref(), Some("Ada"));
    let settings = stored.imap.unwrap();
    assert_eq!(settings.sign_in, SignInMethod::Password);
    assert_eq!(settings.imap.username, USER, "the address by default");
    assert_eq!(
        f.secrets.get(&ada()).await.unwrap(),
        ImapSecret::password(PASSWORD)
    );
}

#[tokio::test]
async fn a_refused_password_adds_nothing_and_says_which_server() {
    let f = fixture().await;
    let refused = f.add(vec!["wrong"]).await.unwrap_err().to_string();
    assert!(refused.contains("nothing was kept"), "{refused}");
    assert!(
        refused.contains("IMAP: it refused the sign-in"),
        "{refused}"
    );
    assert!(f.store.account(&ada()).unwrap().is_none());
    assert!(f.secrets.get(&ada()).await.is_err());
}

#[test]
fn a_password_is_never_an_argument() {
    let line = ["postrinch-cli", "add-imap", USER, "--password", "x"];
    assert!(Cli::try_parse_from(line).is_err());
    // Servers come both or neither.
    let line = [
        "postrinch-cli",
        "add-imap",
        USER,
        "--imap-host",
        "imap.example.org",
    ];
    assert!(Cli::try_parse_from(line).is_err());
}

fn candidate(host: &str, port: u16) -> Candidate {
    Candidate {
        host: host.into(),
        port,
        security: postrinch_discover::Security::Tls,
        username: Username::LocalPart,
    }
}

#[test]
fn discovered_servers_are_used_unless_nobody_vouches_for_them() {
    let mut found = Discovery::nothing();
    found.imap = vec![candidate("imap.elsewhere.net", 993)];
    found.smtp = vec![candidate("smtp.elsewhere.net", 465)];
    let settings = settings_from_discovery(&args(&[USER]), &ada(), &found).unwrap();
    assert_eq!(settings.imap.host, "imap.elsewhere.net");
    assert_eq!(settings.smtp.port, 465);
    assert_eq!(settings.imap.username, "ada", "as the candidate says");

    found.confirm_servers = true;
    let refused = settings_from_discovery(&args(&[USER]), &ada(), &found).unwrap_err();
    assert!(
        refused.to_string().contains("--accept-servers"),
        "{refused}"
    );
    let accepted = settings_from_discovery(&args(&[USER, "--accept-servers"]), &ada(), &found);
    assert!(accepted.is_ok());

    let nothing = settings_from_discovery(&args(&[USER]), &ada(), &Discovery::nothing());
    assert!(nothing.unwrap_err().to_string().contains("--imap-host"));
}
