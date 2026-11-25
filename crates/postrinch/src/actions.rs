//! What the window does to mail and accounts: through the commands for the
//! store, and on sync's runtime for anything that talks to an account's
//! provider.

use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use postrinch_commands::Commands;
use postrinch_model::{EmailAddress, LabelPrefs, Provider, ProviderMessageId, SignInMethod};
use postrinch_sync::google::{GoogleAuth, remove_google_account, sign_in_google};
use postrinch_sync::{SyncError, SyncResult};

use crate::state::State;
use crate::sync::SyncHandle;

/// What the window acts through. The window's state is all `Copy` signals;
/// these are the two things it shares that are not, set once at startup.
struct Services {
    commands: Rc<Commands>,
    sync: Option<SyncHandle>,
}

thread_local! {
    static SERVICES: OnceCell<Services> = const { OnceCell::new() };
    /// Messages being downloaded because the person opened them, so one
    /// download is not asked for twice.
    static DOWNLOADING: RefCell<HashSet<(String, String)>> = RefCell::new(HashSet::new());
}

/// Set what the window acts through. Called once, before anything acts.
/// Forget downloads that have finished, so a message can be asked for
/// again if it failed.
pub fn finished(keys: &[(String, String)]) {
    DOWNLOADING.with(|d| {
        let mut d = d.borrow_mut();
        for key in keys {
            d.remove(key);
        }
    });
}

pub fn init(commands: Rc<Commands>, sync: Option<SyncHandle>) {
    SERVICES.with(|s| {
        if s.set(Services { commands, sync }).is_err() {
            tracing::error!("the window's services were set twice");
        }
    });
}

pub(crate) fn commands() -> Option<Rc<Commands>> {
    SERVICES.with(|s| s.get().map(|s| s.commands.clone()))
}

pub(crate) fn sync() -> Option<SyncHandle> {
    SERVICES.with(|s| s.get().and_then(|s| s.sync.clone()))
}

/// Something an account action wants the person to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub trouble: bool,
}

impl State {
    /// Save what the person changed about a label, and show it.
    pub fn save_label_prefs(self, account: &EmailAddress, label: &str, prefs: &LabelPrefs) {
        let Some(commands) = commands() else {
            return;
        };
        match commands.set_label_prefs(account, label, prefs) {
            Ok(()) => self.revision.update(|n| *n += 1),
            Err(e) => self.error.set(Some(e.to_string())),
        }
    }

    /// Start sync's next pass now.
    pub fn sync_now(self) {
        match sync() {
            Some(sync) => sync.wake.notify_one(),
            None => self.notice(sync_off()),
        }
    }

    /// Sign an account in through the browser: a new Google account, or
    /// `again` for one that has to sign in again, whatever its provider.
    pub fn sign_in(self, again: Option<EmailAddress>) {
        let Some(sync) = sync() else {
            return self.notice(sync_off());
        };
        let route = match sign_in_route(&sync, again.as_ref()) {
            Ok(route) => route,
            Err(refused) => return self.notice(refused),
        };
        self.notice(Notice {
            text: "Finish signing in in your browser.".into(),
            trouble: false,
        });
        let (notice, revision) = (self.notice, self.revision);
        sync.runtime.clone().spawn(async move {
            let opened = match route {
                SignInRoute::Google(auth) => {
                    sign_in_google(&auth, &sync.store, again.as_ref(), |url| {
                        open::that_detached(url)
                    })
                    .await
                }
                SignInRoute::Imap(account) => {
                    let browser = Box::new(|url: &str| open::that_detached(url));
                    let imap = sync.mail().imap();
                    imap.sign_in_again(&sync.imap_oauth, &account, browser)
                        .await
                }
            };
            let text = match opened {
                Ok(account) => {
                    // Gmail's filters are read again, as a new account's are.
                    if let Some(gmail) = sync.mail().gmail() {
                        gmail.recheck_rules(&account.address);
                    }
                    sync.wake.notify_one();
                    Notice {
                        text: format!("{} is signed in. Its mail is on the way.", account.address),
                        trouble: false,
                    }
                }
                Err(e) => Notice {
                    text: format!("Signing in did not finish: {e}"),
                    trouble: true,
                },
            };
            notice.send(Some(text));
            revision.update_send(|n| *n += 1);
        });
    }

    /// Remove an account and its mail from this computer.
    pub fn remove_account(self, account: EmailAddress) {
        let Some(sync) = sync() else {
            return self.notice(sync_off());
        };
        let (notice, revision) = (self.notice, self.revision);
        sync.runtime.clone().spawn(async move {
            let text = match remove(&sync, &account).await {
                Ok(_) => Notice {
                    text: format!("{account} is removed from this computer."),
                    trouble: false,
                },
                Err(e) => Notice {
                    text: format!("{account} could not be removed: {e}"),
                    trouble: true,
                },
            };
            notice.send(Some(text));
            revision.update_send(|n| *n += 1);
        });
    }

    /// Download these messages now, because the person opened them.
    /// Messages already on their way are left alone.
    pub fn download_now(self, account: EmailAddress, ids: Vec<ProviderMessageId>) {
        let Some(sync) = sync() else {
            return;
        };
        let ids: Vec<ProviderMessageId> = DOWNLOADING.with(|d| {
            let mut d = d.borrow_mut();
            ids.into_iter()
                .filter(|id| d.insert((account.key(), id.0.clone())))
                .collect()
        });
        if ids.is_empty() {
            return;
        }
        let (revision, error, done) = (self.revision, self.error, self.downloaded);
        sync.runtime.clone().spawn(async move {
            let keys: Vec<(String, String)> =
                ids.iter().map(|id| (account.key(), id.0.clone())).collect();
            if let Err(e) = sync.mail().download_now(&account, ids).await {
                error.send(Some(format!("The message did not download: {e}")));
            }
            done.send(keys);
            revision.update_send(|n| *n += 1);
        });
    }

    fn notice(self, notice: Notice) {
        self.notice.set(Some(notice));
    }
}

fn sync_off() -> Notice {
    Notice {
        text: "Sync is off, so accounts cannot sign in. The sidebar says why, and the log has the detail.".into(),
        trouble: true,
    }
}

fn google_off() -> Notice {
    Notice {
        text: "Google sign-in is not set up in this copy of Postrinch, so Google accounts cannot sign in. The sidebar says why.".into(),
        trouble: true,
    }
}

/// Where an account signs in.
enum SignInRoute {
    /// Google's sign-in: a new account, or a Google account again.
    Google(Arc<GoogleAuth>),
    /// An IMAP account's provider, in the browser, again.
    Imap(EmailAddress),
}

/// Where `again` (or a new account) signs in; a notice saying why it
/// cannot when it cannot.
fn sign_in_route(sync: &SyncHandle, again: Option<&EmailAddress>) -> Result<SignInRoute, Notice> {
    let stored = match again.map(|address| sync.store.account(address)).transpose() {
        Ok(stored) => stored.flatten(),
        Err(e) => {
            return Err(Notice {
                text: format!("The account could not be read: {e}"),
                trouble: true,
            });
        }
    };
    match stored {
        Some(account) if account.provider == Provider::Imap => {
            let oauth = account
                .imap
                .as_ref()
                .is_some_and(|s| matches!(s.sign_in, SignInMethod::OAuth2 { .. }));
            if oauth {
                Ok(SignInRoute::Imap(account.address))
            } else {
                Err(Notice {
                    text: format!(
                        "{0} signs in with a password. Give it again with `postrinch-cli sign-in {0}`.",
                        account.address
                    ),
                    trouble: true,
                })
            }
        }
        _ => sync
            .auth
            .clone()
            .map(SignInRoute::Google)
            .ok_or_else(google_off),
    }
}

/// Removes the account through its provider: signed out there, its secret
/// forgotten, and its mail gone from this computer.
async fn remove(sync: &SyncHandle, account: &EmailAddress) -> SyncResult<usize> {
    match sync.mail().account(account).await?.provider {
        Provider::Imap => {
            let imap = sync.mail().imap();
            imap.remove_account(&sync.imap_oauth, account).await
        }
        Provider::Google => {
            let auth = sync.auth.as_ref().ok_or_else(|| SyncError::OAuthNotSetUp {
                provider: "Google".into(),
                detail: None,
            })?;
            remove_google_account(auth, &sync.store, account).await
        }
    }
}
