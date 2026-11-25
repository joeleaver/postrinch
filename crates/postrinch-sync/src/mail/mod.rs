//! One sync for every account, whatever its provider (docs/imap.md, "The
//! provider façade"): [`MailSync`] holds the Gmail engine, when Google
//! sign-in is set up, and the IMAP engine always, and sends each call to
//! the account's provider. A copy of Postrinch without Google's OAuth
//! client turns off Google accounts only; IMAP accounts keep working.
//!
//! Keeping every account in sync, and sending what is made here, is
//! [`MailSync::keep_in_sync`] and [`MailSync::keep_sending`] (`keep`):
//! Gmail's loops as they were, and per IMAP account loops that wait out a
//! busy server (`imap_keep`, and the IMAP engine's `BusyGate`).

mod imap_account;
mod imap_idle;
mod imap_keep;
mod keep;

use postrinch_model::{Account, EmailAddress, ImapSearch, Label, Provider, ProviderMessageId};
use postrinch_store::{Store, StoreResult};

use crate::google::GmailSync;
use crate::imap::{ImapSync, ServerFound};
use crate::{ProgressFn, SendReport, SyncError, SyncReport, SyncResult};

/// Sync for every account, routed by provider. Cheap to clone.
#[derive(Clone)]
pub struct MailSync {
    store: Store,
    gmail: Option<GmailSync>,
    imap: ImapSync,
}

/// The engine an account's calls go to.
enum Engine<'a> {
    Gmail(&'a GmailSync),
    Imap(&'a ImapSync),
}

impl MailSync {
    /// Sync over `gmail` for Google accounts (none when Google sign-in is
    /// not set up in this copy: Google accounts then fail with
    /// [`SyncError::OAuthNotSetUp`]) and `imap` for IMAP accounts.
    pub fn new(store: Store, gmail: Option<GmailSync>, imap: ImapSync) -> Self {
        Self { store, gmail, imap }
    }

    /// Waits out busy servers from `first`, doubling up to `cap`, in place
    /// of [`crate::Backoff::new`]'s (for tests; see
    /// [`ImapSync::with_busy_backoff`]).
    pub fn with_busy_backoff(
        mut self,
        first: std::time::Duration,
        cap: std::time::Duration,
    ) -> Self {
        self.imap = self.imap.with_busy_backoff(first, cap);
        self
    }

    /// The Gmail engine, for what only Google accounts have (Gmail's
    /// search and filters); none when Google sign-in is not set up.
    pub fn gmail(&self) -> Option<&GmailSync> {
        self.gmail.as_ref()
    }

    /// The IMAP engine.
    pub fn imap(&self) -> &ImapSync {
        &self.imap
    }

    /// The account's Google Calendar, through the Gmail engine. An IMAP
    /// account has none ([`SyncError::NoCalendars`]).
    pub async fn calendar(&self, account: &EmailAddress) -> SyncResult<&GmailSync> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => Ok(gmail),
            Engine::Imap(_) => Err(SyncError::NoCalendars {
                address: account.clone(),
            }),
        }
    }

    /// Brings the store up to date with the account. When the provider
    /// refuses the account's sign-in, the account is marked as needing to
    /// sign in and [`SyncError::NeedsSignIn`] is returned.
    pub async fn sync(
        &self,
        account: &EmailAddress,
        progress: ProgressFn,
    ) -> SyncResult<SyncReport> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => gmail.sync(account, progress).await,
            Engine::Imap(imap) => imap.sync(account, progress).await,
        }
    }

    /// Downloads every message the store has only headers for, newest
    /// first.
    pub async fn backfill(
        &self,
        account: &EmailAddress,
        progress: ProgressFn,
    ) -> SyncResult<SyncReport> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => gmail.backfill(account, progress).await,
            Engine::Imap(imap) => imap.backfill(account, progress).await,
        }
    }

    /// Downloads these messages now: the person is waiting for them.
    /// Returns how many were stored whole.
    pub async fn download_now(
        &self,
        account: &EmailAddress,
        ids: Vec<ProviderMessageId>,
    ) -> SyncResult<usize> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => gmail.download_now(account, ids).await,
            Engine::Imap(imap) => imap.download_now(account, ids).await,
        }
    }

    /// Asks the account's server to search, for mail not downloaded yet:
    /// Gmail with `gmail` (Gmail's search language), an IMAP server with
    /// `imap` (none asks nothing; docs/imap.md, "Searching the server").
    /// At most `max` messages, newest first, each stored by its headers
    /// at least.
    pub async fn search_server(
        &self,
        account: &EmailAddress,
        gmail: &str,
        imap: Option<&ImapSearch>,
        max: usize,
    ) -> SyncResult<ServerFound> {
        match self.engine(account).await? {
            Engine::Gmail(sync) => Ok(ServerFound {
                ids: sync.search_remote(account, gmail, max).await?,
                incomplete: None,
            }),
            Engine::Imap(sync) => match imap {
                Some(search) => sync.search_server(account, search, max).await,
                None => Ok(ServerFound::default()),
            },
        }
    }

    /// How many of the account's changes and messages wait to be sent.
    pub async fn waiting(&self, account: &EmailAddress) -> SyncResult<usize> {
        Ok(match self.engine(account).await? {
            Engine::Gmail(gmail) => {
                gmail.waiting(account).await? + gmail.outgoing_waiting(account).await?
            }
            Engine::Imap(imap) => {
                imap.waiting(account).await? + imap.outgoing_waiting(account).await?
            }
        })
    }

    /// Sends the account's waiting changes (read, star, labels…), oldest
    /// first.
    pub async fn send_outbox(&self, account: &EmailAddress) -> SyncResult<SendReport> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => gmail.send_outbox(account).await,
            Engine::Imap(imap) => imap.send_outbox(account).await,
        }
    }

    /// Delivers the account's waiting mail: messages sent, drafts kept.
    /// Returns how many went.
    pub async fn send_outgoing(&self, account: &EmailAddress) -> SyncResult<usize> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => gmail.send_outgoing(account).await,
            Engine::Imap(imap) => imap.send_outgoing(account).await,
        }
    }

    /// Makes a label (an IMAP folder) in the account, at once.
    pub async fn create_label(&self, account: &EmailAddress, name: &str) -> SyncResult<Label> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => gmail.create_label(account, name).await,
            Engine::Imap(imap) => imap.create_label(account, name).await,
        }
    }

    /// Renames one of the account's labels, at once; it keeps its id.
    pub async fn rename_label(
        &self,
        account: &EmailAddress,
        id: &str,
        name: &str,
    ) -> SyncResult<Label> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => gmail.rename_label(account, id, name).await,
            Engine::Imap(imap) => imap.rename_label(account, id, name).await,
        }
    }

    /// Deletes one of the account's labels, at once. Its mail stays in
    /// the account (an IMAP folder's is saved to the archive first).
    pub async fn delete_label(&self, account: &EmailAddress, id: &str) -> SyncResult<()> {
        match self.engine(account).await? {
            Engine::Gmail(gmail) => gmail.delete_label(account, id).await,
            Engine::Imap(imap) => imap.delete_label(account, id).await,
        }
    }

    /// Lets go of whatever the account holds open (an IMAP account's
    /// connections), as when it is removed or its settings change. An
    /// account the store no longer has may still hold connections, so
    /// this asks no provider of the store.
    pub async fn disconnect(&self, account: &EmailAddress) {
        self.imap.disconnect(account).await;
    }

    /// The engine for the account's provider.
    async fn engine(&self, account: &EmailAddress) -> SyncResult<Engine<'_>> {
        match self.provider(account).await? {
            Provider::Google => {
                self.gmail
                    .as_ref()
                    .map(Engine::Gmail)
                    .ok_or_else(|| SyncError::OAuthNotSetUp {
                        provider: "Google".into(),
                        detail: None,
                    })
            }
            Provider::Imap => Ok(Engine::Imap(&self.imap)),
        }
    }

    /// The account as the store has it; an error naming it when the store
    /// has none.
    pub async fn account(&self, account: &EmailAddress) -> SyncResult<Account> {
        let a = account.clone();
        self.with_store(move |s| s.account(&a))
            .await?
            .ok_or_else(|| SyncError::Config(format!("{account} is not an account here")))
    }

    /// The account's provider, as the store has it.
    async fn provider(&self, account: &EmailAddress) -> SyncResult<Provider> {
        Ok(self.account(account).await?.provider)
    }

    /// Runs store calls on a blocking thread: every store call may wait
    /// on the disk.
    async fn with_store<T: Send + 'static>(
        &self,
        call: impl FnOnce(&Store) -> StoreResult<T> + Send + 'static,
    ) -> SyncResult<T> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || call(&store)).await {
            Ok(result) => Ok(result?),
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(e) => Err(SyncError::Io(std::io::Error::other(e))),
        }
    }
}
