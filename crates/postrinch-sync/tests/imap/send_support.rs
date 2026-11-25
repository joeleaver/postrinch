//! What the sending tests share: a fake IMAP server with the standard
//! folders and a fake SMTP server, both for one account, a store that has
//! synced the folders once, and the sync, which looks in Sent briefly.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use postrinch_mailtest::{
    Extension, ImapConfig, ImapServer, MessageInfo, NewMessage, SmtpConfig, SmtpFault, SmtpServer,
    SpecialUse, UserHandle,
};
use postrinch_model::{
    Account, Draft, EmailAddress, ImapSettings, Mailbox, Provider, ProviderMessageId, Security,
    SignInMethod,
};
use postrinch_store::{Outgoing, OutgoingKind, SentBackOff, Store};
use postrinch_sync::MemorySecrets;
use postrinch_sync::ProgressFn;
use postrinch_sync::imap::{
    Connector, ImapSecret, ImapSecrets, ImapSync, SentLookup, Timeouts, TlsConfig, message_id,
};

use super::support::{PASSWORD, Tokens, USER, account, server, settings};

pub const TO: &str = "bob@example.net";
pub const CC: &str = "carol@example.net";
pub const BCC: &str = "eve@example.net";

pub struct SendFixture {
    pub imap: ImapServer,
    pub smtp: SmtpServer,
    pub user: UserHandle,
    pub store: Store,
    pub sync: ImapSync,
    pub me: EmailAddress,
    _dir: tempfile::TempDir,
}

/// A send fixture whose IMAP server offers everything (`uidplus`), or
/// everything but UIDPLUS; `server_files_sent` is the account's setting.
pub async fn send_fixture(uidplus: bool, server_files_sent: bool) -> SendFixture {
    send_fixture_with(uidplus, server_files_sent, SmtpConfig::new()).await
}

/// As [`send_fixture`], with the SMTP server set up as `smtp` says (its
/// user is added).
pub async fn send_fixture_with(
    uidplus: bool,
    server_files_sent: bool,
    smtp: SmtpConfig,
) -> SendFixture {
    let folders = |user: &UserHandle| user.create_standard_folders().unwrap();
    fixture(uidplus, server_files_sent, smtp, folders).await
}

/// As [`send_fixture`] (with UIDPLUS, the setting off), for an account
/// with a Sent folder and no Drafts.
pub async fn send_fixture_without_drafts() -> SendFixture {
    let folders = |user: &UserHandle| {
        user.create_special_folder("Sent", SpecialUse::Sent)
            .unwrap();
    };
    fixture(true, false, SmtpConfig::new(), folders).await
}

async fn fixture(
    uidplus: bool,
    server_files_sent: bool,
    smtp: SmtpConfig,
    folders: impl FnOnce(&UserHandle),
) -> SendFixture {
    let mut config = ImapConfig::new();
    if !uidplus {
        config = config.without(Extension::Uidplus);
    }
    let imap = ImapServer::start(config.user(USER, PASSWORD))
        .await
        .unwrap();
    let smtp = SmtpServer::start(smtp.user(USER, PASSWORD)).await.unwrap();
    let user = imap.user(USER);
    folders(&user);
    let me = account();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut settings = settings(
        server(imap.tls_addr().unwrap(), Security::Tls),
        server(smtp.tls_addr().unwrap(), Security::Tls),
        SignInMethod::Password,
    );
    settings.server_files_sent = server_files_sent;
    let mut stored = Account::new(me.clone(), Provider::Imap);
    stored.imap = Some(settings);
    store.put_account(&stored).unwrap();
    let secrets = ImapSecrets::new(Arc::new(MemorySecrets::default()));
    secrets
        .set(&me, &ImapSecret::password(PASSWORD))
        .await
        .unwrap();
    let roots = vec![
        imap.certificate().der().clone(),
        smtp.certificate().der().clone(),
    ];
    let connector =
        Connector::new(TlsConfig::with_extra_roots(roots).unwrap()).with_timeouts(Timeouts {
            connect: Duration::from_secs(5),
            command: Duration::from_secs(5),
        });
    let sync = ImapSync::new(store.clone(), connector, secrets, Arc::new(Tokens::none()))
        .with_sent_lookup(lookup(SentBackOff::default()));
    let fixture = SendFixture {
        imap,
        smtp,
        user,
        store,
        sync,
        me,
        _dir: dir,
    };
    fixture.sync().await;
    fixture
}

fn quiet() -> ProgressFn {
    Arc::new(|_| {})
}

/// Looks in Sent briefly, and gives a provider's setting back after
/// `back_off`.
fn lookup(back_off: SentBackOff) -> SentLookup {
    SentLookup {
        waits: vec![Duration::from_millis(50)],
        back_off,
    }
}

impl SendFixture {
    /// The fixture, giving a provider's setting back after `back_off`.
    pub fn backing_off(mut self, back_off: SentBackOff) -> Self {
        self.sync = self.sync.with_sent_lookup(lookup(back_off));
        self
    }

    pub async fn sync(&self) {
        self.sync.sync(&self.me, quiet()).await.unwrap();
    }

    /// A draft to [`TO`], blind-copied to [`BCC`].
    pub fn draft(&self, subject: &str) -> Draft {
        let mut draft = Draft::new(self.me.clone());
        draft.to = vec![Mailbox::new(None, EmailAddress::parse(TO).unwrap())];
        draft.bcc = vec![Mailbox::new(None, EmailAddress::parse(BCC).unwrap())];
        draft.subject = subject.into();
        draft.body = format!("The body of {subject}.");
        draft
    }

    /// A draft to [`TO`], copied to [`CC`] and blind-copied to [`BCC`].
    pub fn draft_for_three(&self, subject: &str) -> Draft {
        let mut draft = self.draft(subject);
        draft.cc = vec![Mailbox::new(None, EmailAddress::parse(CC).unwrap())];
        draft
    }

    pub fn queue(&self, draft: &Draft, kind: OutgoingKind) -> u64 {
        self.store.queue_outgoing(draft, kind).unwrap()
    }

    pub fn outgoing(&self) -> Vec<Outgoing> {
        self.store.outgoing_for(&self.me).unwrap()
    }

    /// The one piece of outgoing mail left.
    pub fn waiting(&self) -> Outgoing {
        let mut left = self.outgoing();
        assert_eq!(left.len(), 1, "{left:?}");
        left.remove(0)
    }

    /// The account's settings as stored now.
    pub fn settings(&self) -> ImapSettings {
        self.store.account(&self.me).unwrap().unwrap().imap.unwrap()
    }

    /// What the fake holds in `folder`, oldest first.
    pub fn folder(&self, folder: &str) -> Vec<MessageInfo> {
        self.user.messages(folder).unwrap()
    }

    /// The provider id of a message as it is on the fake.
    pub fn id_of(&self, message: &MessageInfo) -> ProviderMessageId {
        message_id(&self.me, &header_of(message))
    }

    /// How many copies the store keeps of `id`.
    pub fn places(&self, id: &ProviderMessageId) -> usize {
        self.store.imap_message_places(&self.me, id).unwrap().len()
    }

    pub fn message_count(&self) -> usize {
        self.store.message_ids(&self.me).unwrap().len()
    }

    /// Plays a provider that files the mail it sends in Sent itself.
    pub fn files_sent_mail(&self) {
        self.files_sent_mail_while(Arc::new(AtomicBool::new(true)));
    }

    /// Plays a provider that files the mail it sends in Sent itself while
    /// `on` is.
    pub fn files_sent_mail_while(&self, on: Arc<AtomicBool>) {
        let user = self.user.clone();
        self.smtp.on_accept(move |message| {
            if !on.load(Ordering::SeqCst) {
                return;
            }
            user.append(
                "Sent",
                NewMessage::new(message.data.clone()).flags(&["\\Seen"]),
            )
            .unwrap();
        });
    }

    /// How many commands the fake IMAP server was sent that start with
    /// `verb`.
    pub fn imap_commands(&self, verb: &str) -> usize {
        self.imap
            .commands()
            .iter()
            .filter(|c| c.starts_with(verb))
            .count()
    }
}

/// A programmed SMTP answer.
pub fn reply(code: u16, text: &str) -> SmtpFault {
    SmtpFault::Reply {
        code,
        text: text.into(),
    }
}

/// A message's `Message-ID`, without brackets.
pub fn header_of(message: &MessageInfo) -> String {
    message_id_header(message).expect("a Message-ID")
}

/// A message's `Message-ID`, without brackets, if it has one.
pub fn message_id_header(message: &MessageInfo) -> Option<String> {
    let value = header(&message.body, "Message-ID")?;
    let id = value.trim().strip_prefix('<')?.strip_suffix('>')?;
    Some(id.to_string())
}

/// The value of the header `name` in a message's own header block.
pub fn header(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let head = text.split("\r\n\r\n").next().unwrap_or_default();
    head.split("\r\n").find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}
