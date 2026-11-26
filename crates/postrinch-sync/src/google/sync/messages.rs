//! Fetching and storing single messages, and running many at once.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use postrinch_model::{EmailAddress, ProviderMessageId, ProviderThreadId, Timestamp};
use postrinch_store::{Store, StoreError};
use tokio::sync::RwLock;
use tokio::task::JoinSet;

use super::{CONCURRENT_FETCHES, GmailSync};
use crate::giving_way::Unfinished;
use crate::google::api::{MetadataMessage, MinimalMessage, Priority, RawMessage};
use crate::google::labels::split_label_ids;
use crate::{Envelope, SyncError, SyncResult, parse_headers, parse_message};
use crate::{Phase, Progress, ProgressFn, SyncReport};

/// How far [`GmailSync::for_each_until`] went.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ran {
    /// The ids it saw through.
    pub(super) through: usize,
    /// Whether it gave way with ids not started.
    pub(super) gave_way: bool,
}

/// What became of one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Stored from its headers alone.
    Headers,
    /// Downloaded whole and stored.
    Fetched,
    Relabelled,
    /// Gmail no longer has it; it was removed from the store.
    Removed,
}

/// The ids of the account's stored labels, reloaded when a message turns
/// out to carry one created since they were read.
pub(super) type KnownLabels = Arc<RwLock<Arc<HashSet<String>>>>;

/// Gmail's `internalDate`, milliseconds since the epoch as a string.
fn received(id: &str, internal_date: &str) -> SyncResult<Timestamp> {
    internal_date
        .parse::<i64>()
        .map(Timestamp::from_millis)
        .map_err(|e| SyncError::Mime(format!("message {id}: internalDate {internal_date:?}: {e}")))
}

fn is_unknown_label(result: &SyncResult<()>) -> bool {
    matches!(
        result,
        Err(SyncError::Store(StoreError::UnknownLabel { .. }))
    )
}

impl GmailSync {
    /// Fetch a message's headers and store it, to be downloaded later.
    pub(super) async fn fetch_headers(
        &self,
        account: &EmailAddress,
        id: &str,
        labels: &KnownLabels,
    ) -> SyncResult<Outcome> {
        let meta = match self.api.metadata_message(account, id).await {
            Ok(meta) => Arc::new(meta),
            Err(e) if e.is_not_found() => return self.remove(account, id).await,
            Err(e) => return Err(e),
        };
        if self.is_settings(account, &meta.label_ids) {
            return self.remove(account, id).await;
        }
        let mut result = self.store_headers(account, &meta, labels).await;
        if is_unknown_label(&result) {
            self.reload_labels(account, labels).await?;
            result = self.store_headers(account, &meta, labels).await;
        }
        result.map(|()| Outcome::Headers)
    }

    async fn store_headers(
        &self,
        account: &EmailAddress,
        meta: &Arc<MetadataMessage>,
        labels: &KnownLabels,
    ) -> SyncResult<()> {
        let known = labels.read().await.clone();
        let (account, meta) = (account.clone(), Arc::clone(meta));
        self.blocking(move |s| {
            let (label_ids, flags) = split_label_ids(&meta.label_ids, &known);
            let envelope = Envelope {
                account,
                id: ProviderMessageId(meta.id.clone()),
                thread: ProviderThreadId(meta.thread_id.clone()),
                received: received(&meta.id, &meta.internal_date)?,
                labels: label_ids,
                flags,
                snippet: Some(meta.snippet.clone()),
            };
            let message = parse_headers(envelope, &meta.header_block(), meta.size_estimate)?;
            Ok(s.put_message(&message, None)?)
        })
        .await
    }

    /// Fetch a message whole and store it.
    pub(super) async fn fetch(
        &self,
        account: &EmailAddress,
        id: &str,
        labels: &KnownLabels,
        priority: Priority,
    ) -> SyncResult<Outcome> {
        let raw = match self.api.raw_message(account, id, priority).await {
            Ok(raw) => raw,
            Err(e) if e.is_not_found() => return self.remove(account, id).await,
            Err(e) => return Err(e),
        };
        if self.is_settings(account, &raw.label_ids) {
            return self.remove(account, id).await;
        }
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
            let (label_ids, flags) = split_label_ids(&raw.label_ids, &known);
            let envelope = Envelope {
                account,
                id: ProviderMessageId(raw.id.clone()),
                thread: ProviderThreadId(raw.thread_id.clone()),
                received: received(&raw.id, &raw.internal_date)?,
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
        if self.is_settings(account, &minimal.label_ids) {
            return self.remove(account, id).await;
        }
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
    /// the rest: none is started after it, and those under way are seen
    /// through.
    pub(super) async fn for_each<F, Fut>(
        &self,
        ids: Vec<String>,
        phase: Phase,
        progress: &ProgressFn,
        report: &mut SyncReport,
        work: F,
    ) -> SyncResult<()>
    where
        F: Fn(GmailSync, String) -> Fut,
        Fut: Future<Output = SyncResult<Outcome>> + Send + 'static,
    {
        self.for_each_until(ids, phase, progress, report, work, || false)
            .await
            .map(|_| ())
            .map_err(|unfinished| unfinished.error)
    }

    /// As [`Self::for_each`], starting no more once `give_way` says so
    /// (it is asked after each id, while some are not started yet): those
    /// under way are seen through, and the rest are left.
    ///
    /// A failure, too, starts no more and sees those under way through,
    /// counting and reporting each, then says how many ids were gone
    /// through. Work under way is never cut off: a download cut off while
    /// it stores its message would store it uncounted, after the failure
    /// was reported, and the count would not match the store.
    pub(super) async fn for_each_until<F, Fut>(
        &self,
        ids: Vec<String>,
        phase: Phase,
        progress: &ProgressFn,
        report: &mut SyncReport,
        work: F,
        mut give_way: impl FnMut() -> bool + Send,
    ) -> Result<Ran, Unfinished>
    where
        F: Fn(GmailSync, String) -> Fut,
        Fut: Future<Output = SyncResult<Outcome>> + Send + 'static,
    {
        let total = ids.len();
        let mut ran = Ran::default();
        let mut failed: Option<SyncError> = None;
        let mut tasks = JoinSet::new();
        let mut ids = ids.into_iter();
        progress(Progress::Working {
            phase,
            done: 0,
            total,
        });
        loop {
            while !ran.gave_way
                && failed.is_none()
                && tasks.len() < CONCURRENT_FETCHES
                && let Some(id) = ids.next()
            {
                tasks.spawn(work(self.clone(), id));
            }
            let Some(joined) = tasks.join_next().await else {
                return match failed {
                    None => Ok(ran),
                    Some(error) => Err(Unfinished {
                        through: ran.through,
                        error,
                    }),
                };
            };
            let outcome = match joined {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(error)) => {
                    note_failure(&mut failed, error);
                    continue;
                }
                Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                Err(e) => {
                    note_failure(&mut failed, SyncError::Io(std::io::Error::other(e)));
                    continue;
                }
            };
            match outcome {
                Outcome::Headers => report.headers += 1,
                Outcome::Fetched => report.fetched += 1,
                Outcome::Relabelled => report.relabelled += 1,
                Outcome::Removed => report.removed += 1,
            }
            ran.through += 1;
            progress(Progress::Working {
                phase,
                done: ran.through,
                total,
            });
            if !ran.gave_way && failed.is_none() && ids.len() > 0 && give_way() {
                ran.gave_way = true;
            }
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

/// Keeps the failure [`GmailSync::for_each_until`]'s work ends with:
/// the first, the one that stopped it, unless work already under way
/// then found the account needs signing in again. That one is kept
/// whatever came before it, as the keep-in-sync loop asks the person to
/// sign in for it rather than retrying later. Any other is logged.
fn note_failure(failed: &mut Option<SyncError>, error: SyncError) {
    let replaces = |first: &SyncError| {
        matches!(error, SyncError::NeedsSignIn(_)) && !matches!(first, SyncError::NeedsSignIn(_))
    };
    match failed {
        Some(first) if !replaces(first) => {
            tracing::warn!("Gmail: also failed, after {first}: {error}");
        }
        Some(first) => {
            tracing::warn!("Gmail: failed, before the account needed signing in: {first}");
            *failed = Some(error);
        }
        None => *failed = Some(error),
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::SignInReason;

    use super::*;

    fn sign_in(detail: &str) -> SyncError {
        SyncError::NeedsSignIn(SignInReason::CredentialRejected {
            detail: detail.into(),
        })
    }

    fn other(message: &str) -> SyncError {
        SyncError::Io(std::io::Error::other(message.to_owned()))
    }

    fn kept(errors: Vec<SyncError>) -> String {
        let mut failed = None;
        for error in errors {
            note_failure(&mut failed, error);
        }
        failed.expect("a failure was noted").to_string()
    }

    #[test]
    fn the_first_failure_is_kept() {
        assert_eq!(
            kept(vec![other("first"), other("second")]),
            other("first").to_string()
        );
        assert_eq!(
            kept(vec![sign_in("first"), sign_in("second")]),
            sign_in("first").to_string()
        );
        assert_eq!(
            kept(vec![sign_in("first"), other("second")]),
            sign_in("first").to_string()
        );
    }

    #[test]
    fn needing_to_sign_in_is_kept_over_any_other_failure() {
        assert_eq!(
            kept(vec![other("first"), sign_in("second"), other("third")]),
            sign_in("second").to_string()
        );
    }
}
