//! The Folders section's list following the store: read when its page
//! opens, and again when a pass of its account ends while it is open,
//! over a store synced from the fake IMAP server of `postrinch-mailtest`.
//! Nothing touches the keychain or a real server.

use std::sync::{Arc, mpsc};
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
use postrinch_sync::{MailSync, Progress, SyncEvent, SyncReport};
use postrinch_sync_core::MemorySecrets;
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

/// The window's thread as the folders' readings see it: each answer
/// waits in a queue until the test runs it, on the thread the signals
/// live on, as `rinch::run_on_main_thread` would.
struct Window {
    answers: mpsc::Receiver<Answer>,
}

impl Window {
    /// Runs the next answer, once its reading is done.
    fn answer(&self) {
        let answer = self.answers.recv_timeout(Duration::from_secs(10));
        answer.expect("the reading answers")();
    }
}

/// Reads on the fixture's runtime and answers on the test's thread.
fn reader(f: &Fixture) -> (Reader, Window) {
    let (send, answers) = mpsc::channel();
    let post = move |answer| send.send(answer).expect("the test waits for it");
    let reader = Reader::new(f.runtime.handle().clone(), f.mail.clone(), post);
    (reader, Window { answers })
}

/// How many readings have been asked for.
fn readings(folders: AccountFolders) -> u64 {
    folders.asked.get().0
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
    let page = Signal::new(None);
    let (reader, window) = reader(&f);
    folders.follow(reader, notice, move || page.get(), reports.ended);
    assert_eq!(readings(folders), 0, "no page is open");

    // The add-account flow opens the account's settings at once.
    page.set(Some(f.ada.clone()));
    assert_eq!(readings(folders), 1);
    window.answer();
    assert_eq!(folders.shown.get(), Some((f.ada.clone(), Vec::new())));

    // The page built again (its account's facts changed) is no opening.
    page.set(Some(f.ada.clone()));
    assert_eq!(readings(folders), 1);

    // The first pass runs; while it does, nothing is read again.
    reports.report(SyncEvent::Started {
        account: f.ada.clone(),
    });
    let pass = f.runtime.block_on(f.mail.sync(&f.ada, Arc::new(|_| {})));
    reports.report(SyncEvent::Progress {
        account: f.ada.clone(),
        progress: Progress::Listing { found: 3 },
    });
    assert_eq!(readings(folders), 1);

    reports.report(SyncEvent::Synced {
        account: f.ada.clone(),
        report: pass.unwrap(),
    });
    assert_eq!(readings(folders), 2, "read once when the pass ended");
    window.answer();
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
    assert_eq!(readings(folders), 2);

    // The next pass of the account is read too, even ending as the last.
    reports.report(SyncEvent::Synced {
        account: f.ada.clone(),
        report: SyncReport::default(),
    });
    assert_eq!(readings(folders), 3);
    window.answer();
}

#[test]
fn a_closed_page_is_no_longer_followed() {
    let f = fixture();
    let folders = AccountFolders::new();
    let notice = Signal::new(None);
    let reports = Reports::new();
    let page = Signal::new(Some(f.ada.clone()));
    let (reader, window) = reader(&f);
    folders.follow(reader, notice, move || page.get(), reports.ended);
    assert_eq!(readings(folders), 1, "the page was open already");
    window.answer();
    assert_eq!(folders.shown.get(), Some((f.ada.clone(), Vec::new())));

    // Settings closes while the reading after a pass is under way: its
    // answer is not shown.
    let pass = f.runtime.block_on(f.mail.sync(&f.ada, Arc::new(|_| {})));
    reports.report(SyncEvent::Synced {
        account: f.ada.clone(),
        report: pass.unwrap(),
    });
    assert_eq!(readings(folders), 2);
    page.set(None);
    window.answer();
    assert_eq!(folders.shown.get(), Some((f.ada.clone(), Vec::new())));

    // Later passes read nothing.
    reports.report(SyncEvent::Synced {
        account: f.ada.clone(),
        report: SyncReport::default(),
    });
    assert_eq!(readings(folders), 2);

    // Opening it again reads it again, as the store has it now.
    page.set(Some(f.ada.clone()));
    assert_eq!(readings(folders), 3);
    window.answer();
    assert_eq!(shown_names(folders).len(), 5);
    assert!(notice.get().is_none());
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
    let failed = || Err(postrinch_sync::SyncError::SignIn("gone".into()));
    folders.answered(notice, other, bob.clone(), failed());
    assert!(notice.get().is_some_and(|n| n.trouble));

    // A reading that fails once its page closed tells nobody.
    notice.set(None);
    let closed = folders.ask(bob.clone());
    folders.unfollow();
    assert!(!folders.follows(&bob));
    folders.answered(notice, closed, bob, failed());
    assert!(notice.get().is_none());
}
