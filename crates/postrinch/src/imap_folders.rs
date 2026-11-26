//! An IMAP account's folders as its settings' Folders section offers them
//! for each role. The list follows what the store has while the account's
//! page is shown: it is read when the page opens, and again whenever a
//! pass of that account ends, as a pass keeps the folders it listed
//! (`ImapSync::pass`). The first pass of an account just added ends after
//! its settings first open, so a list read only at opening would stay
//! empty. Once the page closes, nothing is read until it opens again.

use std::sync::Arc;

use postrinch_model::EmailAddress;
use postrinch_sync::imap::AccountFolder;
use postrinch_sync::{MailSync, SyncResult};
use rinch::prelude::*;
use rinch::{Effect, untracked};
use tokio::runtime::Handle;

use crate::actions::{Notice, sync};
use crate::state::State;
use crate::sync::{PassEnd, pass_ended};
use crate::sync_start::SyncHandle;

/// An answer to hand back to the window's thread.
type Answer = Box<dyn FnOnce() + Send>;

/// Where a reading of the folders runs, and how its answer gets back to
/// the window's thread: sync's runtime and `rinch::run_on_main_thread` in
/// the app; a test's own runtime and queue in its tests.
#[derive(Clone)]
pub struct Reader {
    runtime: Handle,
    mail: MailSync,
    post: Arc<dyn Fn(Answer) + Send + Sync>,
}

impl Reader {
    /// Reads on sync's runtime, answering on the window's thread.
    pub fn of(sync: &SyncHandle) -> Self {
        Self::new(sync.runtime.clone(), sync.mail().clone(), |answer| {
            rinch::run_on_main_thread(answer)
        })
    }

    /// Reads with `mail` on `runtime`, handing each answer to `post`,
    /// which must run it on the thread the signals live on.
    pub(crate) fn new(
        runtime: Handle,
        mail: MailSync,
        post: impl Fn(Answer) + Send + Sync + 'static,
    ) -> Self {
        Self {
            runtime,
            mail,
            post: Arc::new(post),
        }
    }
}

/// The folders the Folders section shows, and which reading of them it
/// waits for.
#[derive(Clone, Copy)]
pub struct AccountFolders {
    /// The account the folders are for, and its folders (no inbox), as
    /// last read.
    pub shown: Signal<Option<(EmailAddress, Vec<AccountFolder>)>>,
    /// The last reading asked for, by number, and the account followed:
    /// none once its page closed. Only the answer to that reading, for
    /// that account, is shown, so a slow early reading cannot put back a
    /// list a later one replaced, and one answering after the page
    /// closed shows nothing (not even its error).
    asked: Signal<(u64, Option<EmailAddress>)>,
}

impl AccountFolders {
    pub fn new() -> Self {
        Self {
            shown: Signal::new(None),
            asked: Signal::new((0, None)),
        }
    }

    /// Follows the account `page` says is shown (none when no IMAP
    /// account's page is): reads its folders with `reader` when its page
    /// opens, not each time the page is built again, and once more each
    /// time a pass of it ends, until the page closes. A reading that
    /// fails says so in `notice`.
    pub fn follow(
        self,
        reader: Reader,
        notice: Signal<Option<Notice>>,
        page: impl Fn() -> Option<EmailAddress> + 'static,
        ended: Signal<Option<PassEnd>>,
    ) {
        let opened = reader.clone();
        Effect::new(move || {
            let page = page();
            untracked(|| match page {
                Some(account) if !self.follows(&account) => self.read(&opened, notice, account),
                Some(_) => {}
                None => self.unfollow(),
            });
        });
        // Once a pass, not while it runs.
        Effect::new(move || {
            let Some(end) = ended.get() else {
                return;
            };
            untracked(|| {
                if self.follows(&end.account) {
                    self.read(&reader, notice, end.account);
                }
            });
        });
    }

    /// Reads `account`'s folders from the store off the window's thread,
    /// and shows them when they come back.
    fn read(self, reader: &Reader, notice: Signal<Option<Notice>>, account: EmailAddress) {
        let asked = self.ask(account.clone());
        let (mail, post) = (reader.mail.clone(), reader.post.clone());
        reader.runtime.spawn(async move {
            let found = mail.imap().account_folders(&account).await;
            post(Box::new(move || {
                self.answered(notice, asked, account, found)
            }));
        });
    }

    /// Notes a reading of `account`'s folders asked for, and returns its
    /// number.
    fn ask(self, account: EmailAddress) -> u64 {
        let number = untracked(|| self.asked.get().0) + 1;
        self.asked.set((number, Some(account)));
        number
    }

    /// Stops following: answers still to come are not shown.
    fn unfollow(self) {
        if untracked(|| self.asked.with(|(_, account)| account.is_some())) {
            self.asked.update(|(_, account)| *account = None);
        }
    }

    /// The reading numbered `asked` answered: shown when it is still the
    /// last one asked for and its account still followed, and only when
    /// the folders changed, so the section is not built again for
    /// nothing.
    fn answered(
        self,
        notice: Signal<Option<Notice>>,
        asked: u64,
        account: EmailAddress,
        found: SyncResult<Vec<AccountFolder>>,
    ) {
        let last = untracked(|| {
            self.asked
                .with(|(number, followed)| *number == asked && followed.as_ref() == Some(&account))
        });
        if !last {
            return;
        }
        match found {
            Ok(found) => self.shown.set_if_changed(Some((account, found))),
            Err(e) => notice.set(Some(Notice {
                text: format!("{account}'s folders could not be read: {e}"),
                trouble: true,
            })),
        }
    }

    /// Whether the section shows, or is reading, `account`'s folders.
    fn follows(self, account: &EmailAddress) -> bool {
        untracked(|| {
            self.asked
                .with(|(_, followed)| followed.as_ref() == Some(account))
        })
    }

    /// Runs a pass of `account` now, so that roles just chosen move to
    /// their folders, and reports its end so the list follows it.
    pub fn reload_after_pass(state: State, account: EmailAddress) {
        let Some(sync) = sync() else {
            return;
        };
        sync.runtime.clone().spawn(async move {
            // The pass waits its turn behind sync's own on the account.
            let pass = sync.mail().sync(&account, Arc::new(|_| {})).await;
            if let Err(e) = pass {
                tracing::info!("IMAP: the pass after a role change did not finish: {e}");
            }
            state.revision.update_send(|n| *n += 1);
            pass_ended(state.pass_ended, account);
        });
    }
}

#[cfg(test)]
#[path = "imap_folders_tests.rs"]
mod tests;
