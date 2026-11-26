//! What the IMAP sync tests share: a fake server with an account on it,
//! a store in a temporary directory with the account in it, and the sync.

use std::sync::Arc;

use postrinch_mailtest::{Extension, ImapConfig, ImapServer, NewMessage, UserHandle};
use postrinch_model::{
    Account, EmailAddress, FolderRoles, Message, Provider, ProviderMessageId, Security,
    SignInMethod,
};
use postrinch_store::Store;
use postrinch_sync::imap::{ImapSecret, ImapSecrets, ImapSync, message_id};
use postrinch_sync::{ProgressFn, SyncReport};
use postrinch_sync_core::MemorySecrets;

use super::support::{PASSWORD, Tokens, USER, account, closed_port, connector, server, settings};

pub struct Fixture {
    pub server: ImapServer,
    pub user: UserHandle,
    pub store: Store,
    pub sync: ImapSync,
    pub me: EmailAddress,
    _dir: tempfile::TempDir,
}

/// A fake offering every extension.
pub async fn fixture() -> Fixture {
    fixture_with(ImapConfig::new(), FolderRoles::default()).await
}

/// A fake withholding these extensions.
pub async fn fixture_without(extensions: &[Extension]) -> Fixture {
    let mut config = ImapConfig::new();
    for extension in extensions {
        config = config.without(*extension);
    }
    fixture_with(config, FolderRoles::default()).await
}

pub async fn fixture_with(config: ImapConfig, roles: FolderRoles) -> Fixture {
    let fake = ImapServer::start(config.user(USER, PASSWORD))
        .await
        .unwrap();
    let user = fake.user(USER);
    let me = account();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut settings = settings(
        server(fake.tls_addr().unwrap(), Security::Tls),
        server(closed_port().await, Security::Tls),
        SignInMethod::Password,
    );
    settings.roles = roles;
    let mut stored = Account::new(me.clone(), Provider::Imap);
    stored.imap = Some(settings);
    store.put_account(&stored).unwrap();
    let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
    secrets
        .set(&me, &ImapSecret::password(PASSWORD))
        .await
        .unwrap();
    let sync = ImapSync::new(
        store.clone(),
        connector(fake.certificate()),
        secrets,
        Arc::new(Tokens::none()),
    );
    Fixture {
        server: fake,
        user,
        store,
        sync,
        me,
        _dir: dir,
    }
}

fn quiet() -> ProgressFn {
    Arc::new(|_| {})
}

/// A message with these headers and a short body.
pub fn mail(message_id: Option<&str>, subject: &str, extra: &str) -> NewMessage {
    let id = message_id
        .map(|id| format!("Message-ID: <{id}>\r\n"))
        .unwrap_or_default();
    let raw = format!(
        "From: Ann <ann@example.com>\r\nTo: ada@example.org\r\nSubject: {subject}\r\n\
         Date: Tue, 24 Nov 2026 09:00:00 +0000\r\n{id}{extra}\r\nThe body of {subject}.\r\n"
    );
    NewMessage::new(raw.into_bytes())
}

impl Fixture {
    pub async fn sync(&self) -> SyncReport {
        self.sync.sync(&self.me, quiet()).await.unwrap()
    }

    pub async fn backfill(&self) -> SyncReport {
        self.sync.backfill(&self.me, quiet()).await.unwrap()
    }

    /// The stored message whose `Message-ID` is `header`.
    pub fn message(&self, header: &str) -> Option<Message> {
        self.store
            .message(&self.me, &message_id(&self.me, header))
            .unwrap()
    }

    pub fn id(&self, header: &str) -> ProviderMessageId {
        message_id(&self.me, header)
    }

    /// The labels of the stored message whose `Message-ID` is `header`.
    pub fn labels(&self, header: &str) -> Vec<String> {
        let mut labels = self.message(header).expect("stored").labels;
        labels.sort();
        labels
    }

    /// The label id the store has for a folder, by its label name.
    pub fn label_id(&self, name: &str) -> String {
        self.store
            .labels(&self.me)
            .unwrap()
            .into_iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("no label {name}"))
            .provider_id
    }

    /// The label id of a folder kept by sync, by its name on the server:
    /// for a folder that is no label (Gmail's All Mail).
    pub fn folder_id(&self, name: &str) -> String {
        self.store
            .imap_folders(&self.me)
            .unwrap()
            .into_iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no folder {name}"))
            .label_id
    }

    pub fn label_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .store
            .labels(&self.me)
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect();
        names.sort();
        names
    }

    pub fn message_count(&self) -> usize {
        self.store.message_ids(&self.me).unwrap().len()
    }
}

/// The UIDs asked for by their headers in these commands (as
/// `ImapServer::commands` shows them), once for each time asked, in
/// order.
pub fn header_fetches(commands: &[String]) -> Vec<u32> {
    let mut uids = Vec::new();
    for command in commands.iter().filter(|c| c.contains("BODY.PEEK[HEADER]")) {
        let set = command
            .strip_prefix("UID FETCH ")
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or_else(|| panic!("not a UID FETCH: {command}"));
        for range in set.split(',') {
            let (a, b) = range.split_once(':').unwrap_or((range, range));
            let (a, b): (u32, u32) = (a.parse().unwrap(), b.parse().unwrap());
            uids.extend(a.min(b)..=a.max(b));
        }
    }
    uids
}
