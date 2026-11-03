//! What the window does to mail and accounts: through the commands for the
//! store, and on sync's runtime for anything that talks to Google.

use std::cell::OnceCell;
use std::rc::Rc;

use postrinch_commands::Commands;
use postrinch_model::{EmailAddress, LabelPrefs};
use postrinch_sync::google::{remove_google_account, sign_in_google};

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
}

/// Set what the window acts through. Called once, before anything acts.
pub fn init(commands: Rc<Commands>, sync: Option<SyncHandle>) {
    SERVICES.with(|s| {
        if s.set(Services { commands, sync }).is_err() {
            tracing::error!("the window's services were set twice");
        }
    });
}

fn commands() -> Option<Rc<Commands>> {
    SERVICES.with(|s| s.get().map(|s| s.commands.clone()))
}

fn sync() -> Option<SyncHandle> {
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

    /// Sign a Google account in through the browser: a new one, or `again`
    /// for one that has to sign in again.
    pub fn sign_in(self, again: Option<EmailAddress>) {
        let Some(sync) = sync() else {
            return self.notice(sync_off());
        };
        self.notice(Notice {
            text: "Finish signing in in your browser.".into(),
            trouble: false,
        });
        let (notice, revision) = (self.notice, self.revision);
        sync.runtime.clone().spawn(async move {
            let opened = sign_in_google(&sync.auth, &sync.store, again.as_ref(), |url| {
                open::that_detached(url)
            })
            .await;
            let text = match opened {
                Ok(account) => {
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
            let text = match remove_google_account(&sync.auth, &sync.store, &account).await {
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

    fn notice(self, notice: Notice) {
        self.notice.set(Some(notice));
    }
}

fn sync_off() -> Notice {
    Notice {
        text: "Sync is off, so accounts cannot sign in. The sidebar says why.".into(),
        trouble: true,
    }
}
