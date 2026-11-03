//! Keeping the store in step with a Gmail account.
//!
//! The first sync of an account (or one after Gmail's history has expired)
//! is a full sync: list every message id, fetch the ones the store lacks,
//! remove the ones Gmail no longer has. After that, sync reads Gmail's
//! history from where it last got to and applies the changes.
//!
//! Where sync got to is the account's sync cursor in the store. It is
//! written only once everything before it is stored, so a sync that is
//! interrupted repeats work when it runs again, and never skips any.

mod backfill;
mod cursor;
mod full;
mod incremental;
mod keep;
mod messages;
mod outbox;

pub use keep::{KeepOptions, SyncEvent, SyncEventFn};
pub use outbox::SendReport;

use std::collections::HashSet;
use std::sync::Arc;

use postrinch_model::{AccountState, EmailAddress};
use postrinch_store::{Store, StoreResult};

use self::cursor::Cursor;
use super::GmailApi;
use super::labels::to_label;
use crate::{SyncError, SyncResult};

/// How many messages are fetched at once. Gmail allows 250 quota units a
/// second per user and a message fetch costs 5.
const CONCURRENT_FETCHES: usize = 10;

/// What a sync did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Whether it listed the whole account rather than reading history.
    pub full: bool,
    /// Messages stored from their headers alone, to be downloaded later
    /// ([`GmailSync::backfill`]).
    pub headers: usize,
    /// Messages downloaded whole and stored.
    pub fetched: usize,
    /// Stored messages whose labels or flags were brought up to date.
    pub relabelled: usize,
    /// Messages removed because Gmail no longer has them.
    pub removed: usize,
}

impl SyncReport {
    fn add(&mut self, other: SyncReport) {
        self.full |= other.full;
        self.headers += other.headers;
        self.fetched += other.fetched;
        self.relabelled += other.relabelled;
        self.removed += other.removed;
    }
}

/// How a sync is getting on, for a progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// A full sync has listed this many message ids so far.
    Listing { found: usize },
    /// Working through messages: `done` of `total` in this phase.
    Working {
        phase: Phase,
        done: usize,
        total: usize,
    },
}

/// What a sync is working through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Storing the headers of messages new to the store.
    Headers,
    /// Applying changes: new messages, labels, deletions.
    Changes,
    /// Downloading whole messages known only by their headers.
    Bodies,
}

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

/// Gmail sync into the store. Cheap to clone.
#[derive(Clone)]
pub struct GmailSync {
    api: Arc<GmailApi>,
    store: Store,
    /// How many of the newest messages a full sync downloads whole.
    whole_first: usize,
}

impl GmailSync {
    pub fn new(api: Arc<GmailApi>, store: Store) -> Self {
        Self {
            api,
            store,
            whole_first: full::WHOLE_FIRST,
        }
    }

    /// Download this many of the newest messages whole in a full sync,
    /// rather than by their headers (by default [`full::WHOLE_FIRST`]).
    pub fn with_whole_first(mut self, count: usize) -> Self {
        self.whole_first = count;
        self
    }

    /// Bring the store up to date with the account. When Gmail no longer
    /// accepts the account's credential, the account is marked as needing
    /// to sign in and [`SyncError::NeedsSignIn`] is returned. Changes sent
    /// from the outbox before it began are forgotten once it succeeds.
    pub async fn sync(
        &self,
        account: &EmailAddress,
        progress: ProgressFn,
    ) -> SyncResult<SyncReport> {
        let started = outbox::now();
        let result = self.sync_from_cursor(account, &progress).await;
        if result.is_ok() {
            // Gmail's report of the messages now includes every change sent
            // before this sync began: those need not be applied again.
            let account = account.clone();
            self.with_store(move |s| s.forget_sent(&account, started))
                .await?;
        }
        if let Err(SyncError::NeedsSignIn(reason)) = &result {
            let state = AccountState::NeedsSignIn(reason.clone());
            let account = account.clone();
            self.with_store(move |s| s.set_account_state(&account, &state))
                .await?;
        }
        result
    }

    async fn sync_from_cursor(
        &self,
        account: &EmailAddress,
        progress: &ProgressFn,
    ) -> SyncResult<SyncReport> {
        let mut report = SyncReport::default();
        let mut refresh_existing = false;
        let mut cursor = self.cursor(account).await?;
        loop {
            let since = match cursor {
                Some(Cursor::History { history_id }) => history_id,
                Some(Cursor::FullSince { history_id }) => {
                    report.add(self.full(account, refresh_existing, progress).await?);
                    history_id
                }
                None => {
                    let since = self.api.profile(account).await?.history_id;
                    self.set_cursor(
                        account,
                        Cursor::FullSince {
                            history_id: since.clone(),
                        },
                    )
                    .await?;
                    report.add(self.full(account, refresh_existing, progress).await?);
                    since
                }
            };
            match self.incremental(account, &since, progress).await {
                Ok(done) => {
                    report.add(done);
                    return Ok(report);
                }
                // Gmail no longer has history that old. Start over from a
                // full listing, and this time refresh the labels of messages
                // already stored: changes to them since `since` are lost.
                Err(e) if e.is_not_found() && !refresh_existing => {
                    tracing::info!("{account}: history from {since} has expired, syncing in full");
                    refresh_existing = true;
                    cursor = None;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Store Gmail's labels, remove stored ones Gmail no longer has, and
    /// return the ids of the labels now stored.
    async fn refresh_labels(&self, account: &EmailAddress) -> SyncResult<Arc<HashSet<String>>> {
        let labels: Vec<_> = self
            .api
            .labels(account)
            .await?
            .iter()
            .filter_map(to_label)
            .collect();
        let account = account.clone();
        self.with_store(move |s| {
            let keep: HashSet<String> = labels.iter().map(|l| l.provider_id.clone()).collect();
            for label in &labels {
                s.put_label(&account, label)?;
            }
            for stale in s.labels(&account)? {
                if !keep.contains(&stale.provider_id) {
                    s.remove_label(&account, &stale.provider_id)?;
                }
            }
            Ok(Arc::new(keep))
        })
        .await
    }

    async fn cursor(&self, account: &EmailAddress) -> SyncResult<Option<Cursor>> {
        let account = account.clone();
        let raw = self.with_store(move |s| s.sync_cursor(&account)).await?;
        raw.map(|r| Cursor::parse(&r)).transpose()
    }

    async fn set_cursor(&self, account: &EmailAddress, cursor: Cursor) -> SyncResult<()> {
        let account = account.clone();
        let raw = cursor.to_string();
        self.with_store(move |s| s.set_sync_cursor(&account, Some(&raw)))
            .await
    }

    /// Run store calls on a blocking thread: every store call may wait on
    /// the disk.
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
