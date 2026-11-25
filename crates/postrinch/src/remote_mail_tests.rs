//! An agent's calls over an IMAP account, as the app serves them: the
//! store synced from the fake IMAP server of `postrinch-mailtest`, the
//! agents' server over a real loopback connection, and [`RemoteMail`] over
//! the [`MailSync`] façade with no Gmail engine. Nothing touches the
//! keychain or a real server.

use std::net::TcpListener;
use std::sync::Arc;

use postrinch_agents::client::Client;
use postrinch_agents::serve::Hooks;
use postrinch_agents::{Agents, Call, Changed, Reach, Remote};
use postrinch_commands::{Commands, Folder, ThreadAction};
use postrinch_mailtest::{ImapConfig, ImapServer, NewMessage, UserHandle};
use postrinch_model::{
    Account, EmailAddress, FolderRoles, ImapSettings, Provider, ProviderThreadId, Security, Server,
    SignInMethod, SystemLabel,
};
use postrinch_store::Store;
use postrinch_sync::imap::{
    Connector, ImapOAuth, ImapSecret, ImapSecrets, ImapSync, OAuthClients, TlsConfig,
};
use postrinch_sync::{MailSync, MemorySecrets};
use tokio::runtime::Runtime;

use super::RemoteMail;

const USER: &str = "ada@example.org";
const PASSWORD: &str = "correct horse";

struct Fixture {
    runtime: Runtime,
    _server: ImapServer,
    user: UserHandle,
    mail: MailSync,
    ada: EmailAddress,
    client: Client,
    _dir: tempfile::TempDir,
}

fn server(host: std::net::SocketAddr) -> Server {
    Server {
        host: host.ip().to_string(),
        port: host.port(),
        security: Security::Tls,
        username: USER.into(),
    }
}

/// A port nothing listens on: the account never sends mail here.
fn closed_port() -> std::net::SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// An IMAP account with one message in its inbox, synced, and an agent
/// that may reach every account.
fn fixture() -> Fixture {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let ada = EmailAddress::parse(USER).unwrap();
    let (server_, user, mail) = runtime.block_on(async {
        let server_ = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
            .await
            .unwrap();
        let user = server_.user(USER);
        let raw = "From: Ann <ann@example.com>\r\nTo: ada@example.org\r\n\
                   Subject: Thursday rota\r\nDate: Tue, 24 Nov 2026 09:00:00 +0000\r\n\
                   Message-ID: <rota@example.com>\r\n\r\nCan you take Thursday?\r\n";
        user.append("INBOX", NewMessage::new(raw.as_bytes().to_vec()))
            .unwrap();
        let mut account = Account::new(ada.clone(), Provider::Imap);
        account.imap = Some(ImapSettings {
            imap: server(server_.tls_addr().unwrap()),
            smtp: server(closed_port()),
            sign_in: SignInMethod::Password,
            roles: FolderRoles::default(),
            server_files_sent: true,
        });
        store.put_account(&account).unwrap();
        let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
        secrets
            .set(&ada, &ImapSecret::password(PASSWORD))
            .await
            .unwrap();
        let oauth = ImapOAuth::new(
            OAuthClients::none(),
            secrets.clone(),
            Arc::new(MemorySecrets::default()),
        )
        .unwrap();
        let roots = vec![server_.certificate().der().clone()];
        let connector = Connector::new(TlsConfig::with_extra_roots(roots).unwrap());
        let imap = ImapSync::new(store.clone(), connector, secrets, Arc::new(oauth));
        let mail = MailSync::new(store.clone(), None, imap);
        // Headers first, then bodies, as the keep-in-sync loop does.
        mail.sync(&ada, Arc::new(|_| {})).await.unwrap();
        mail.backfill(&ada, Arc::new(|_| {})).await.unwrap();
        (server_, user, mail)
    });

    let agents_path = dir.path().join("agents.json");
    let mut agents = Agents::load(&agents_path).unwrap();
    let (_, key) = agents.create("Test agent", Reach::Every).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let remote = RemoteMail::new(runtime.handle().clone(), mail.clone());
    postrinch_agents::serve::start_on(
        listener,
        Commands::new(store),
        agents_path,
        Hooks {
            on_change: Arc::new(|_: &Changed| {}),
            remote: Some(Arc::new(remote)),
        },
    )
    .unwrap();
    Fixture {
        runtime,
        _server: server_,
        user,
        mail,
        ada,
        client: Client::new(addr, Some(key)),
        _dir: dir,
    }
}

fn folder_names(user: &UserHandle) -> Vec<String> {
    user.folders()
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect()
}

#[test]
fn an_agent_lists_reads_and_changes_an_imap_accounts_mail() {
    let f = fixture();
    let page = f
        .client
        .call(&Call::ListThreads {
            folder: Folder::Everywhere(SystemLabel::Inbox),
            account: None,
            category: None,
            offset: 0,
            limit: 10,
        })
        .unwrap();
    assert_eq!(page["total"], 1, "{page}");
    assert_eq!(page["threads"][0]["untrusted"]["subject"], "Thursday rota");
    let thread = ProviderThreadId(page["threads"][0]["thread"].as_str().unwrap().into());

    let read = f
        .client
        .call(&Call::ReadThread {
            account: f.ada.clone(),
            thread: thread.clone(),
        })
        .unwrap();
    let body = read["messages"][0]["untrusted"]["body"]
        .as_str()
        .unwrap_or_else(|| panic!("{read}"));
    assert!(body.contains("Can you take Thursday?"), "{read}");

    // Starred here, then sent: the server's copy is flagged.
    f.client
        .call(&Call::Act {
            threads: vec![(f.ada.clone(), thread)],
            search: None,
            messages: Vec::new(),
            actions: vec![ThreadAction::Star],
        })
        .unwrap();
    assert!(!f.user.messages("INBOX").unwrap()[0].has_flag("\\Flagged"));
    let sent = f.runtime.block_on(f.mail.send_outbox(&f.ada)).unwrap();
    assert!(sent.refused.is_empty(), "{sent:?}");
    assert!(f.user.messages("INBOX").unwrap()[0].has_flag("\\Flagged"));
}

#[test]
fn an_agent_makes_renames_and_deletes_an_imap_accounts_folders() {
    let f = fixture();
    let made = f
        .client
        .call(&Call::CreateLabel {
            account: f.ada.clone(),
            name: "Plans".into(),
        })
        .unwrap();
    assert_eq!(made["made"], true, "{made}");
    assert!(folder_names(&f.user).contains(&"Plans".to_owned()));

    f.client
        .call(&Call::RenameLabel {
            account: f.ada.clone(),
            label: "Plans".into(),
            name: "Ideas".into(),
        })
        .unwrap();
    let names = folder_names(&f.user);
    assert!(names.contains(&"Ideas".to_owned()), "{names:?}");
    assert!(!names.contains(&"Plans".to_owned()), "{names:?}");

    f.client
        .call(&Call::DeleteLabel {
            account: f.ada.clone(),
            label: "Ideas".into(),
        })
        .unwrap();
    assert!(!folder_names(&f.user).contains(&"Ideas".to_owned()));
}

#[test]
fn an_imap_account_has_no_calendar_to_change() {
    let f = fixture();
    let refused = f
        .client
        .call(&Call::CalendarShares {
            account: f.ada.clone(),
            calendar: USER.into(),
        })
        .unwrap_err();
    assert!(refused.contains("is an IMAP account"), "{refused}");

    // Asked of the provider itself, the façade refuses it too.
    let remote = RemoteMail::new(f.runtime.handle().clone(), f.mail.clone());
    let refused = remote.calendar_shares(&f.ada, USER).unwrap_err();
    assert_eq!(
        refused,
        "ada@example.org is an IMAP account, and calendars are Google accounts' only."
    );
}
