//! An IMAP account's folders as its settings' Folders section offers them
//! for each role. The list follows what the store has: it is read when the
//! section opens, and again whenever a pass of that account ends, as a
//! pass keeps the folders it listed (`ImapSync::pass`). The first pass of
//! an account just added ends after its settings first open, so a list
//! read only at opening would stay empty.

use std::sync::Arc;

use postrinch_model::EmailAddress;
use postrinch_sync::SyncResult;
use postrinch_sync::imap::AccountFolder;
use rinch::prelude::*;
use rinch::{Effect, untracked};

use crate::actions::{Notice, sync};
use crate::state::State;
use crate::sync::{PassEnd, pass_ended};

/// The folders the Folders section shows, and which reading of them it
/// waits for.
#[derive(Clone, Copy)]
pub struct AccountFolders {
    /// The account the folders are for, and its folders (no inbox), as
    /// last read.
    pub shown: Signal<Option<(EmailAddress, Vec<AccountFolder>)>>,
    /// The last reading asked for: its number and its account. Only its
    /// answer is shown, so a slow early reading cannot put back a list a
    /// later one replaced.
    asked: Signal<(u64, Option<EmailAddress>)>,
}

impl AccountFolders {
    pub fn new() -> Self {
        Self {
            shown: Signal::new(None),
            asked: Signal::new((0, None)),
        }
    }

    /// Reads `account`'s folders from the store, for the section's
    /// choices.
    pub fn load(self, state: State, account: EmailAddress) {
        let Some(sync) = sync() else {
            return;
        };
        let asked = self.ask(account.clone());
        let notice = state.notice;
        sync.runtime.clone().spawn(async move {
            let found = sync.mail().imap().account_folders(&account).await;
            rinch::run_on_main_thread(move || self.answered(notice, asked, account, found));
        });
    }

    /// Notes a reading of `account`'s folders asked for, and returns its
    /// number.
    pub(crate) fn ask(self, account: EmailAddress) -> u64 {
        let number = untracked(|| self.asked.get().0) + 1;
        self.asked.set((number, Some(account)));
        number
    }

    /// The reading numbered `asked` answered: shown when it is still the
    /// last one asked for, and only when the folders changed, so the
    /// section is not built again for nothing.
    pub(crate) fn answered(
        self,
        notice: Signal<Option<Notice>>,
        asked: u64,
        account: EmailAddress,
        found: SyncResult<Vec<AccountFolder>>,
    ) {
        if untracked(|| self.asked.get().0) != asked {
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
    pub(crate) fn follows(self, account: &EmailAddress) -> bool {
        untracked(|| {
            self.asked
                .with(|(_, asked)| asked.as_ref() == Some(account))
        })
    }

    /// Reads the folders again with `load` each time a pass of the account
    /// they are for ends: once a pass, not while it runs.
    pub fn follow_passes(
        self,
        ended: Signal<Option<PassEnd>>,
        load: impl Fn(EmailAddress) + 'static,
    ) {
        Effect::new(move || {
            let Some(end) = ended.get() else {
                return;
            };
            untracked(|| {
                if self.follows(&end.account) {
                    load(end.account);
                }
            });
        });
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
