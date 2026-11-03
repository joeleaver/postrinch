//! Fetching and storing single messages, and running many at once.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use postrinch_model::{EmailAddress, ProviderMessageId, ProviderThreadId, Timestamp};
use postrinch_store::{Store, StoreError};
use tokio::sync::RwLock;
use tokio::task::JoinSet;

use super::{CONCURRENT_FETCHES, GmailSync, Progress, ProgressFn, SyncReport};
use crate::google::api::{MinimalMessage, RawMessage};
use crate::google::labels::split_label_ids;
use crate::{Envelope, SyncError, SyncResult, parse_message};

/// What became of one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    Fetched,
    Relabelled,
    /// Gmail no longer has it; it was removed from the store.
    Removed,
}

/// The ids of the account's stored labels, reloaded when a message turns
/// out to carry one created since they were read.
pub(super) type KnownLabels = Arc<RwLock<Arc<HashSet<String>>>>;

fn is_unknown_label(result: &SyncResult<()>) -> bool {
    matches!(
        result,
        Err(SyncError::Store(StoreError::UnknownLabel { .. }))
    )
}

impl GmailSync {
    /// Fetch a message in full and store it.
    pub(super) async fn fetch(
        &self,
        account: &EmailAddress,
        id: &str,
        labels: &KnownLabels,
    ) -> SyncResult<Outcome> {
        let raw = match self.api.raw_message(account, id).await {
            Ok(raw) => raw,
            Err(e) if e.is_not_found() => return self.remove(account, id).await,
            Err(e) => return Err(e),
        };
        let raw = Arc::new(raw);
        let mut result = self.store_raw(account, &raw, labels).await;
        if is_unknown_label(&result) {
            self.reload_labels(account, labels).await?;
            result = self.store_raw(account, &raw, labels).await;
        }
        result.map(|()| Outcome::Fetched)
    }

    async fn store_raw(
        &self,
        account: &EmailAddress,
        raw: &Arc<RawMessage>,
        labels: &KnownLabels,
    ) -> SyncResult<()> {
        let known = labels.read().await.clone();
        let (account, raw) = (account.clone(), Arc::clone(raw));
        self.blocking(move |s| {
            let bytes = raw.bytes()?;
            let received = raw.internal_date.parse::<i64>().map_err(|e| {
                SyncError::Mime(format!(
                    "message {}: internalDate {:?}: {e}",
                    raw.id, raw.internal_date
                ))
            })?;
            let (label_ids, flags) = split_label_ids(&raw.label_ids, &known);
            let envelope = Envelope {
                account,
                id: ProviderMessageId(raw.id.clone()),
                thread: ProviderThreadId(raw.thread_id.clone()),
                received: Timestamp::from_millis(received),
                labels: label_ids,
                flags,
                snippet: Some(raw.snippet.clone()),
            };
            let message = parse_message(envelope, &bytes)?;
            Ok(s.put_message(&message, Some(&bytes))?)
        })
        .await
    }

    /// Bring a stored message's labels and flags up to date.
    pub(super) async fn relabel(
        &self,
        account: &EmailAddress,
        id: &str,
        labels: &KnownLabels,
    ) -> SyncResult<Outcome> {
        let minimal = match self.api.minimal_message(account, id).await {
            Ok(m) => Arc::new(m),
            Err(e) if e.is_not_found() => return self.remove(account, id).await,
            Err(e) => return Err(e),
        };
        let mut result = self.store_labels(account, &minimal, labels).await;
        if is_unknown_label(&result) {
            self.reload_labels(account, labels).await?;
            result = self.store_labels(account, &minimal, labels).await;
        }
        result.map(|()| Outcome::Relabelled)
    }

    async fn store_labels(
        &self,
        account: &EmailAddress,
        m: &Arc<MinimalMessage>,
        labels: &KnownLabels,
    ) -> SyncResult<()> {
        let known = labels.read().await.clone();
        let (account, m) = (account.clone(), Arc::clone(m));
        self.blocking(move |s| {
            let (label_ids, flags) = split_label_ids(&m.label_ids, &known);
            Ok(s.set_message_labels(
                &account,
                &ProviderMessageId(m.id.clone()),
                &label_ids,
                flags,
            )?)
        })
        .await
    }

    pub(super) async fn remove(&self, account: &EmailAddress, id: &str) -> SyncResult<Outcome> {
        let (a, mid) = (account.clone(), ProviderMessageId(id.to_owned()));
        self.with_store(move |s| s.remove_message(&a, &mid)).await?;
        Ok(Outcome::Removed)
    }

    async fn reload_labels(&self, account: &EmailAddress, labels: &KnownLabels) -> SyncResult<()> {
        let fresh = self.refresh_labels(account).await?;
        *labels.write().await = fresh;
        Ok(())
    }

    /// Run `work` on every id, [`CONCURRENT_FETCHES`] at a time, reporting
    /// progress and tallying outcomes into `report`. The first error stops
    /// the rest.
    pub(super) async fn for_each<F, Fut>(
        &self,
        ids: Vec<String>,
        progress: &ProgressFn,
        report: &mut SyncReport,
        work: F,
    ) -> SyncResult<()>
    where
        F: Fn(GmailSync, String) -> Fut,
        Fut: Future<Output = SyncResult<Outcome>> + Send + 'static,
    {
        let total = ids.len();
        let mut done = 0;
        let mut tasks = JoinSet::new();
        let mut ids = ids.into_iter();
        progress(Progress::Applying { done, total });
        loop {
            while tasks.len() < CONCURRENT_FETCHES
                && let Some(id) = ids.next()
            {
                tasks.spawn(work(self.clone(), id));
            }
            let Some(joined) = tasks.join_next().await else {
                return Ok(());
            };
            match joined {
                Ok(Ok(Outcome::Fetched)) => report.fetched += 1,
                Ok(Ok(Outcome::Relabelled)) => report.relabelled += 1,
                Ok(Ok(Outcome::Removed)) => report.removed += 1,
                Ok(Err(e)) => return Err(e),
                Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                Err(e) => return Err(SyncError::Io(std::io::Error::other(e))),
            }
            done += 1;
            progress(Progress::Applying { done, total });
        }
    }

    /// Run a call that uses the store and may fail with a sync error on a
    /// blocking thread.
    async fn blocking<T: Send + 'static>(
        &self,
        call: impl FnOnce(&Store) -> SyncResult<T> + Send + 'static,
    ) -> SyncResult<T> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || call(&store)).await {
            Ok(result) => result,
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(e) => Err(SyncError::Io(std::io::Error::other(e))),
        }
    }
}
