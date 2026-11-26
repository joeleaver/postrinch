//! The Folders section's list following the store: read when it opens,
//! and again when a pass of its account ends, over a store synced from
//! the fake IMAP server of `postrinch-mailtest`. Nothing touches the
//! keychain or a real server.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use postrinch_mailtest::{ImapConfig, ImapServer};
use postrinch_model::{
    Account, FolderRole, FolderRoles, ImapSettings, Provider, Security, Server, SignInMethod,
    SystemLabel,
};
use postrinch_store::Store;
use postrinch_sync::imap::{
    Connector, ImapOAuth, ImapSecret, ImapSecrets, ImapSync, OAuthClients, TlsConfig,
};
use postrinch_sync::{MailSync, MemorySecrets, Progress, SyncEvent, SyncReport};
use tokio::runtime::Runtime;
use tokio::sync::Notify;

use super::*;
use crate::sync::report;

const USER: &str = "ada@example.org";
const PASSWORD: &str = "correct horse";

struct Fixture {
    runtime: Runtime,
    _server: ImapServer,
    mail: MailSync,
    ada: EmailAddress,
    _dir: tempfile::TempDir,
}

/// An IMAP account just added, with Sent, Drafts, Trash, Junk and Archive
/// on its server, and no pass run yet: the store has none of its folders.
fn fixture() -> Fixture {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let ada = EmailAddress::parse(USER).unwrap();
    let (server, mail) = runtime.block_on(async {
        let server = ImapServer::start(ImapConfig::new().user(USER, PASSWORD))
            .await
            .unwrap();
        server.user(USER).create_standard_folders().unwrap();
        let at = |addr: std::net::SocketAddr| Server {
            host: addr.ip().to_string(),
            port: addr.port(),
            security: Security::Tls,
            username: USER.into(),
        };
        let mut account = Account::new(ada.clone(), Provider::Imap);
        account.imap = Some(ImapSettings {
            imap: at(server.tls_addr().unwrap()),
            smtp: at(server.tls_addr().unwrap()),
            sign_in: SignInMethod::Password,
            roles: FolderRoles::default(),
            server_files_sent: true,
            server_files_sent_chosen: false,
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
        let roots = vec![server.certificate().der().clone()];
        let connector = Connector::new(TlsConfig::with_extra_roots(roots).unwrap());
        let imap = ImapSync::new(store.clone(), connector, secrets, Arc::new(oauth));
        (server, MailSync::new(store, None, imap))
    });
    Fixture {
        runtime,
        _server: server,
        mail,
        ada,
        _dir: dir,
    }
}

/// What the window does to read the folders, here reading them at once.
fn reader(
    f: &Fixture,
    folders: AccountFolders,
    notice: Signal<Option<Notice>>,
    reads: Rc<Cell<usize>>,
) -> impl Fn(EmailAddress) + 'static {
    let (handle, mail) = (f.runtime.handle().clone(), f.mail.clone());
    move |account: EmailAddress| {
        reads.set(reads.get() + 1);
        let asked = folders.ask(account.clone());
        let found = handle.block_on(mail.imap().account_folders(&account));
        folders.answered(notice, asked, account, found);
    }
}

/// Sync's report of an event, as the sync thread makes it.
struct Reports {
    status: Signal<Vec<crate::sync::AccountSync>>,
    revision: Signal<u64>,
    ended: Signal<Option<PassEnd>>,
    wake: Notify,
}

impl Reports {
    fn new() -> Self {
        Self {
            status: Signal::new(Vec::new()),
            revision: Signal::new(0),
            ended: Signal::new(None),
            wake: Notify::new(),
        }
    }

    fn report(&self, event: SyncEvent) {
        report(event, self.status, self.revision, self.ended, &self.wake);
    }
}

fn shown_names(folders: AccountFolders) -> Vec<String> {
    let shown = folders.shown.get().map(|(_, f)| f).unwrap_or_default();
    shown.into_iter().map(|f| f.name).collect()
}

#[test]
fn folders_opened_before_the_first_pass_fill_once_it_ends() {
    let f = fixture();
    let folders = AccountFolders::new();
    let notice = Signal::new(None);
    let reports = Reports::new();
    let reads = Rc::new(Cell::new(0));
    let read = reader(&f, folders, notice, reads.clone());
    folders.follow_passes(reports.ended, reader(&f, folders, notice, reads.clone()));
    assert_eq!(reads.get(), 0, "no pass has ended yet");

    // The add-account flow opens the account's settings at once.
    read(f.ada.clone());
    assert_eq!(folders.shown.get(), Some((f.ada.clone(), Vec::new())));

    // The first pass runs; while it does, nothing is read again.
    reports.report(SyncEvent::Started {
        account: f.ada.clone(),
    });
    let pass = f.runtime.block_on(f.mail.sync(&f.ada, Arc::new(|_| {})));
    reports.report(SyncEvent::Progress {
        account: f.ada.clone(),
        progress: Progress::Listing { found: 3 },
    });
    assert_eq!(reads.get(), 1);

    reports.report(SyncEvent::Synced {
        account: f.ada.clone(),
        report: pass.unwrap(),
    });
    assert_eq!(reads.get(), 2, "read once when the pass ended");
    assert_eq!(
        shown_names(folders),
        ["Archive", "Drafts", "Junk", "Sent", "Trash"]
    );
    let shown = folders.shown.get().unwrap().1;
    let sent = shown.iter().find(|f| f.name == "Sent").unwrap();
    assert_eq!(sent.role, Some(FolderRole::System(SystemLabel::Sent)));
    assert!(notice.get().is_none());

    // Its download ending is no new pass; another account's pass is not
    // this one's.
    reports.report(SyncEvent::Done {
        account: f.ada.clone(),
        downloaded: 0,
    });
    reports.report(SyncEvent::Synced {
        account: EmailAddress::parse("bob@example.org").unwrap(),
        report: SyncReport::default(),
    });
    assert_eq!(reads.get(), 2);

    // The next pass of the account is read too, even ending as the last.
    reports.report(SyncEvent::Synced {
        account: f.ada.clone(),
        report: SyncReport::default(),
    });
    assert_eq!(reads.get(), 3);
}

#[test]
fn a_pass_that_stops_short_is_read_too() {
    let reports = Reports::new();
    let ada = EmailAddress::parse(USER).unwrap();
    let ends = [
        SyncEvent::Failed {
            account: ada.clone(),
            error: "lost".into(),
            retry_in: Duration::from_secs(20),
        },
        SyncEvent::Busy {
            account: ada.clone(),
            message: String::new(),
            retry_in: Duration::from_secs(20),
        },
        SyncEvent::NeedsSignIn {
            account: ada.clone(),
        },
    ];
    for (number, end) in (1..).zip(ends) {
        reports.report(end);
        assert_eq!(
            reports.ended.get(),
            Some(PassEnd {
                account: ada.clone(),
                number
            })
        );
    }
}

#[test]
fn only_the_last_reading_asked_for_is_shown() {
    let folders = AccountFolders::new();
    let notice = Signal::new(None);
    let ada = EmailAddress::parse(USER).unwrap();
    let bob = EmailAddress::parse("bob@example.org").unwrap();
    let sent = AccountFolder {
        name: "Sent".into(),
        shown: "Sent".into(),
        role: None,
    };
    assert!(!folders.follows(&ada));
    let early = folders.ask(ada.clone());
    let late = folders.ask(ada.clone());
    assert!(folders.follows(&ada));
    folders.answered(notice, late, ada.clone(), Ok(vec![sent.clone()]));
    folders.answered(notice, early, ada.clone(), Ok(Vec::new()));
    assert_eq!(folders.shown.get(), Some((ada.clone(), vec![sent])));

    // Opening another account follows it instead.
    let other = folders.ask(bob.clone());
    assert!(folders.follows(&bob) && !folders.follows(&ada));
    let failed = Err(postrinch_sync::SyncError::SignIn("gone".into()));
    folders.answered(notice, other, bob, failed);
    assert!(notice.get().is_some_and(|n| n.trouble));
}
