//! The smoke test against a real IMAP and SMTP server, GreenMail
//! (<https://greenmail-mail-test.github.io/greenmail/>), rather than the
//! in-repo fake of `postrinch-mailtest`: a third party's reading of the
//! RFCs, so what both Postrinch and the fake took for granted is checked
//! against someone else's server.
//!
//! One account, through the code the window and `postrinch-cli` use: it is
//! added with a password (the check of both servers, the secret kept),
//! synced, its mail read, starred, archived, trashed and moved (each seen
//! on the server by another client), mail sent over SMTP arrives and is
//! filed in Sent, and the server's search finds mail not downloaded yet.
//!
//! It needs Java and GreenMail's standalone jar, so it is `#[ignore]`d:
//! `tools/greenmail-smoke.sh` downloads the jar, checks its SHA-256 and
//! runs it (docs/imap.md, "Testing"); CI runs that script. Run with
//! `--ignored` and no jar, it fails saying so: it never passes without a
//! server. GreenMail listens on loopback only; the secrets are kept in
//! memory and the store in a temporary directory.

mod peer;
mod server;

use std::sync::Arc;
use std::time::Duration;

use postrinch_model::{
    Draft, EmailAddress, FolderRoles, ImapAlternative, ImapSearch, ImapSettings, Mailbox,
    MessageChange, Provider, ProviderMessageId, SearchKey, Security, Server, SignInMethod,
};
use postrinch_store::{OutgoingKind, Store};
use postrinch_sync::imap::{AccessTokens, TokenFuture};
use postrinch_sync::imap::{
    Added, Connector, ImapOAuth, ImapSecret, ImapSecrets, ImapSync, NewImapAccount, NewSignIn,
    OAuthClients, SentLookup, Timeouts, TlsConfig, message_id,
};
use postrinch_sync::{MailSync, MemorySecrets, ProgressFn, SyncResult};

use peer::Peer;
use server::{GreenMail, User};

const ADA: User = User {
    address: "ada@example.org",
    password: "correct horse battery",
};
const BOB: User = User {
    address: "bob@example.net",
    password: "staple",
};

/// A provider's usual folders, by the names providers use (GreenMail has
/// no SPECIAL-USE, so Postrinch finds them by name), and one of the
/// person's own.
const FOLDERS: [&str; 6] = ["Sent", "Drafts", "Trash", "Junk", "Archive", "Work"];

/// One test, in steps, as the account's life goes: each step needs what
/// the one before left on the server.
#[tokio::test]
#[ignore = "needs Java and GreenMail: run tools/greenmail-smoke.sh"]
async fn an_imap_account_round_trips_through_greenmail() {
    let dir = tempfile::tempdir().unwrap();
    let greenmail = GreenMail::start(dir.path(), &[ADA, BOB]);
    let mut other = Peer::sign_in(greenmail.imaps, &greenmail.ca, ADA.address, ADA.password).await;
    for folder in FOLDERS {
        other.create(folder).await;
    }
    for (id, subject) in [
        ("a@smoke", "Rota"),
        ("b@smoke", "Minutes"),
        ("c@smoke", "Offer"),
    ] {
        other
            .append("INBOX", id, subject, &format!("The {subject}."))
            .await;
    }

    let client = Client::new(dir.path(), &greenmail);
    client.add().await;
    client.sync().await;
    for id in ["a@smoke", "b@smoke", "c@smoke"] {
        assert_eq!(client.labels(id), ["INBOX"], "{id} is in the inbox");
    }

    changes_reach_the_server(&client, &mut other).await;
    mail_sent_arrives(&client, &greenmail).await;
    the_server_finds_mail_not_here_yet(&client, &mut other).await;
    other.sign_out().await;
}

/// Read, star, archive, trash and move, each made here, sent, and seen
/// on the server by the other client; a sync after agrees.
async fn changes_reach_the_server(client: &Client, other: &mut Peer) {
    client.change("a@smoke", MessageChange::read()).await;
    client.change("a@smoke", MessageChange::starred(true)).await;
    let a = &other.find("INBOX", &["a@smoke"]).await[0];
    assert!(a.has("\\Seen") && a.has("\\Flagged"), "{a:?}");

    client
        .change("b@smoke", MessageChange::remove_label("INBOX"))
        .await;
    assert_eq!(other.ids("Archive").await, ["b@smoke"]);

    client
        .change("c@smoke", MessageChange::add_label("TRASH"))
        .await;
    assert_eq!(other.ids("Trash").await, ["c@smoke"]);

    let work = client.label_id("Work");
    let to_work = MessageChange {
        add_labels: vec![work.clone()],
        remove_labels: vec!["INBOX".into()],
        ..MessageChange::default()
    };
    client.change("a@smoke", to_work).await;
    let moved = &other.find("Work", &["a@smoke"]).await[0];
    assert!(
        moved.has("\\Seen") && moved.has("\\Flagged"),
        "flags go with it: {moved:?}"
    );
    assert!(other.ids("INBOX").await.is_empty(), "the inbox is empty");

    client.sync().await;
    assert_eq!(client.labels("a@smoke"), [work]);
    assert_eq!(client.labels("b@smoke"), [client.label_id("Archive")]);
    assert_eq!(client.labels("c@smoke"), ["TRASH"]);
    let a = client.message("a@smoke");
    assert!(!a.flags.unread && a.flags.starred, "{:?}", a.flags);
}

/// Mail sent here goes over SMTP to Bob's inbox, and a copy is filed in
/// Ada's Sent (GreenMail does not file sent mail itself).
async fn mail_sent_arrives(client: &Client, greenmail: &GreenMail) {
    let mut draft = Draft::new(client.me.clone());
    draft.to = vec![Mailbox::new(
        None,
        EmailAddress::parse(BOB.address).unwrap(),
    )];
    draft.subject = "Smoke signal".into();
    draft.body = "Sent through GreenMail.".into();
    client
        .store
        .queue_outgoing(&draft, OutgoingKind::Send)
        .unwrap();
    assert_eq!(client.mail.send_outgoing(&client.me).await.unwrap(), 1);
    assert!(client.store.outgoing_for(&client.me).unwrap().is_empty());

    let mut bob = Peer::sign_in(greenmail.imaps, &greenmail.ca, BOB.address, BOB.password).await;
    let inbox = bob.messages("INBOX").await;
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert_eq!(inbox[0].subject, "Smoke signal");
    bob.sign_out().await;

    let mut ada = Peer::sign_in(greenmail.imaps, &greenmail.ca, ADA.address, ADA.password).await;
    let sent = ada.messages("Sent").await;
    assert_eq!(sent.len(), 1, "one copy in Sent: {sent:?}");
    assert_eq!(sent[0].subject, "Smoke signal");
    ada.sign_out().await;
}

/// Mail that came after the last sync, its word only in the body, is
/// found by the server's search and stored; mail without the word, and
/// mail in the trash, is not.
async fn the_server_finds_mail_not_here_yet(client: &Client, other: &mut Peer) {
    other
        .append("Work", "d@smoke", "Travel", "Bound for zanzibar.")
        .await;
    other
        .append("INBOX", "e@smoke", "Lunch", "Soup again.")
        .await;
    other
        .append("Trash", "f@smoke", "Old trip", "Zanzibar, last year.")
        .await;
    assert!(client.stored("d@smoke").is_none(), "not synced yet");
    let search = ImapSearch {
        alternatives: vec![ImapAlternative {
            folders: Vec::new(),
            not_in: Vec::new(),
            set_aside_too: false,
            key: SearchKey::Text("zanzibar".into()),
            gmail: "zanzibar".into(),
            inexact: None,
        }],
    };
    let found = client
        .mail
        .search_server(&client.me, "zanzibar", Some(&search), 50)
        .await
        .unwrap();
    assert_eq!(found.incomplete, None);
    assert_eq!(found.ids, [client.id("d@smoke")]);
    assert_eq!(client.message("d@smoke").subject(), "Travel");
}

/// Postrinch's side: a store in a temporary directory, the secrets in
/// memory, and the façade the window uses, with the IMAP engine alone.
struct Client {
    store: Store,
    imap: ImapSync,
    mail: MailSync,
    secrets: ImapSecrets,
    me: EmailAddress,
    greenmail_imaps: Server,
    greenmail_smtps: Server,
}

impl Client {
    fn new(dir: &std::path::Path, greenmail: &GreenMail) -> Self {
        let store = Store::open(dir.join("store")).unwrap();
        let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
        let tls = TlsConfig::with_extra_roots(vec![greenmail.ca.clone()]).unwrap();
        let connector = Connector::new(tls).with_timeouts(Timeouts {
            connect: Duration::from_secs(10),
            command: Duration::from_secs(30),
        });
        let imap = ImapSync::new(store.clone(), connector, secrets.clone(), Arc::new(NoOAuth))
            .with_sent_lookup(SentLookup {
                waits: vec![Duration::from_millis(200)],
                ..SentLookup::default()
            });
        let mail = MailSync::new(store.clone(), None, imap.clone());
        let at = |addr: std::net::SocketAddr| Server {
            host: addr.ip().to_string(),
            port: addr.port(),
            security: Security::Tls,
            username: ADA.address.into(),
        };
        Self {
            store,
            imap,
            mail,
            secrets,
            me: EmailAddress::parse(ADA.address).unwrap(),
            greenmail_imaps: at(greenmail.imaps),
            greenmail_smtps: at(greenmail.smtps),
        }
    }

    /// Adds the account with its password, as the add-account dialog
    /// does, and checks what was kept.
    async fn add(&self) {
        let oauth = ImapOAuth::new(
            OAuthClients::none(),
            self.secrets.clone(),
            Arc::new(MemorySecrets::default()),
        )
        .unwrap();
        let new = NewImapAccount {
            address: self.me.clone(),
            display_name: Some("Ada".into()),
            settings: ImapSettings {
                imap: self.greenmail_imaps.clone(),
                smtp: self.greenmail_smtps.clone(),
                sign_in: SignInMethod::Password,
                roles: FolderRoles::default(),
                server_files_sent: false,
                server_files_sent_chosen: true,
            },
        };
        let password = NewSignIn::Password(ImapSecret::password(ADA.password));
        match self.imap.add_account(&oauth, new, password).await.unwrap() {
            Added::Account(account) => assert_eq!(account.provider, Provider::Imap),
            Added::CheckFailed(check) => panic!("GreenMail failed the check: {check}"),
        }
        let stored = self.store.account(&self.me).unwrap().expect("stored");
        assert!(stored.is_active());
        assert_eq!(
            self.secrets.get(&self.me).await.unwrap(),
            ImapSecret::password(ADA.password)
        );
    }

    async fn sync(&self) {
        let quiet: ProgressFn = Arc::new(|_| {});
        self.mail.sync(&self.me, quiet).await.unwrap();
    }

    /// Makes `change` to one message as the window does, and sends it.
    async fn change(&self, header: &str, change: MessageChange) {
        self.store
            .change_messages(&self.me, &[self.id(header)], &change)
            .unwrap();
        self.mail.send_outbox(&self.me).await.unwrap();
        assert_eq!(
            self.mail.waiting(&self.me).await.unwrap(),
            0,
            "{header}: sent"
        );
    }

    fn id(&self, header: &str) -> ProviderMessageId {
        message_id(&self.me, header)
    }

    fn stored(&self, header: &str) -> Option<postrinch_model::Message> {
        self.store.message(&self.me, &self.id(header)).unwrap()
    }

    fn message(&self, header: &str) -> postrinch_model::Message {
        self.stored(header)
            .unwrap_or_else(|| panic!("{header} is not stored"))
    }

    fn labels(&self, header: &str) -> Vec<String> {
        let mut labels = self.message(header).labels;
        labels.sort();
        labels
    }

    fn label_id(&self, name: &str) -> String {
        self.store
            .labels(&self.me)
            .unwrap()
            .into_iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("no label {name}"))
            .provider_id
    }
}

/// A password account asks for no access token; one asked for is a bug.
struct NoOAuth;

impl AccessTokens for NoOAuth {
    fn access_token<'a>(
        &'a self,
        account: &'a EmailAddress,
    ) -> TokenFuture<'a, SyncResult<String>> {
        panic!("a password account asked for {account}'s access token")
    }

    fn invalidate<'a>(&'a self, _: &'a EmailAddress) -> TokenFuture<'a, ()> {
        Box::pin(async {})
    }
}
