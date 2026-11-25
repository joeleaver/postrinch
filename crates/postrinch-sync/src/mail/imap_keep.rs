//! Keeping IMAP accounts in sync, and sending what is made here: a task
//! per signed-in IMAP account for each duty, so accounts never wait on
//! one another. A pass of one account can hold its sync connection for
//! long (a first sync), and a send can hold its outgoing connection for
//! up to about 15 s while it looks in Sent for mail a provider files
//! itself; doing accounts one after another would make every other
//! account wait behind that.
//!
//! The tasks are kept by [`ImapKeeper::keep`], which starts one for each
//! IMAP account that is signed in and has none, at the start, on each
//! wake (an account signed in again, one added) and every
//! [`KeepOptions::interval`]. A task ends when its account is removed or
//! has to sign in again; dropping `keep` stops them all.

use std::collections::HashMap;
use std::sync::Arc;

use postrinch_model::{EmailAddress, Provider};
use postrinch_store::Store;
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::imap::{BusyGate, ImapSync};
use crate::{KeepOptions, SyncEventFn};

/// What a keeper's tasks do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Duty {
    /// Sync passes, then backfill ([`ImapKeeper::keep_account_in_sync`]).
    Sync,
    /// The outbox, then outgoing mail ([`ImapKeeper::keep_account_sending`]).
    Send,
}

/// The IMAP accounts' loops for one duty. Cheap to clone: each task has
/// its own copy.
#[derive(Clone)]
pub(super) struct ImapKeeper {
    pub(super) sync: ImapSync,
    pub(super) store: Store,
    pub(super) busy: BusyGate,
    pub(super) options: KeepOptions,
    pub(super) events: SyncEventFn,
}

impl ImapKeeper {
    /// Runs a task for `duty` per signed-in IMAP account until dropped.
    /// Each wake of `wake` is passed to every task.
    pub(super) async fn keep(&self, duty: Duty, wake: Arc<Notify>) {
        let mut running: HashMap<String, Arc<Notify>> = HashMap::new();
        let mut tasks: JoinSet<String> = JoinSet::new();
        loop {
            self.start_missing(duty, &mut running, &mut tasks).await;
            tokio::select! {
                () = wake.notified() => {
                    for account_wake in running.values() {
                        account_wake.notify_one();
                    }
                }
                () = tokio::time::sleep(self.options.interval) => {}
                Some(ended) = tasks.join_next() => match ended {
                    Ok(key) => {
                        running.remove(&key);
                    }
                    Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                    Err(e) => tracing::error!("an IMAP account's {duty:?} task ended: {e}"),
                },
            }
        }
    }

    /// Starts a task for each signed-in IMAP account that has none.
    async fn start_missing(
        &self,
        duty: Duty,
        running: &mut HashMap<String, Arc<Notify>>,
        tasks: &mut JoinSet<String>,
    ) {
        let accounts = match self.imap_accounts().await {
            Ok(accounts) => accounts,
            Err(e) => {
                tracing::error!("cannot read the IMAP accounts to keep: {e}");
                return;
            }
        };
        for account in accounts {
            let key = account.key();
            if running.contains_key(&key) {
                continue;
            }
            let wake = Arc::new(Notify::new());
            running.insert(key.clone(), wake.clone());
            let keeper = self.clone();
            tasks.spawn(async move {
                match duty {
                    Duty::Sync => keeper.keep_account_in_sync(&account, wake).await,
                    Duty::Send => keeper.keep_account_sending(&account, wake).await,
                }
                key
            });
        }
    }

    /// The IMAP accounts that are signed in.
    async fn imap_accounts(&self) -> crate::SyncResult<Vec<EmailAddress>> {
        let store = self.store.clone();
        let accounts = self.blocking(move || store.accounts()).await?;
        Ok(accounts
            .into_iter()
            .filter(|a| a.provider == Provider::Imap && a.is_active())
            .map(|a| a.address)
            .collect())
    }

    /// Whether the account is still here and signed in: a task whose
    /// account is not ends. A store that cannot say keeps it going.
    pub(super) async fn still_kept(&self, account: &EmailAddress) -> bool {
        let (store, a) = (self.store.clone(), account.clone());
        match self.blocking(move || store.account(&a)).await {
            Ok(stored) => stored.is_some_and(|s| s.provider == Provider::Imap && s.is_active()),
            Err(e) => {
                tracing::warn!("{account}: cannot read the account: {e}");
                true
            }
        }
    }

    async fn blocking<T: Send + 'static>(
        &self,
        call: impl FnOnce() -> postrinch_store::StoreResult<T> + Send + 'static,
    ) -> crate::SyncResult<T> {
        match tokio::task::spawn_blocking(call).await {
            Ok(result) => Ok(result?),
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(e) => Err(crate::SyncError::Io(std::io::Error::other(e))),
        }
    }
}
