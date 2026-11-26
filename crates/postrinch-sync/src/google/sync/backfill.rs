//! Backfill: downloading the bodies of messages known only by their headers.

use std::sync::Arc;

use postrinch_model::{EmailAddress, ProviderMessageId};
use tokio::sync::RwLock;

use super::GmailSync;
use crate::SyncResult;
use crate::giving_way::{Backfilled, Unfinished};
use crate::google::api::Priority;
use crate::{Phase, ProgressFn, SyncReport};

impl GmailSync {
    /// Download every message the store has only headers for, newest
    /// first. It runs at background priority: it spends only quota that
    /// foreground work leaves, keeping a reserve so a sync or a message the
    /// person opens is never stuck behind it. Stopping it at any point is
    /// safe; the next call carries on from the newest message still left.
    pub async fn backfill(
        &self,
        account: &EmailAddress,
        progress: ProgressFn,
    ) -> SyncResult<SyncReport> {
        Ok(self
            .backfill_until(account, progress, || false)
            .await
            .map_err(|unfinished| unfinished.error)?
            .report)
    }

    /// As [`Self::backfill`], starting no more downloads once `give_way`
    /// says a pass is due (it is asked after each message, while some are
    /// not started yet); those under way are seen through. A failure
    /// says how many messages were gone through before it.
    pub(crate) async fn backfill_until(
        &self,
        account: &EmailAddress,
        progress: ProgressFn,
        give_way: impl FnMut() -> bool + Send,
    ) -> Result<Backfilled, Unfinished> {
        let mut out = Backfilled::default();
        let a = account.clone();
        let ids: Vec<String> = self
            .with_store(move |s| s.undownloaded(&a))
            .await?
            .into_iter()
            .map(|id| id.0)
            .collect();
        if ids.is_empty() {
            return Ok(out);
        }
        let labels = Arc::new(RwLock::new(self.refresh_labels(account).await?));
        let account_for = account.clone();
        let ran = self
            .for_each_until(
                ids,
                Phase::Bodies,
                &progress,
                &mut out.report,
                move |sync, id| {
                    let (account, labels) = (account_for.clone(), Arc::clone(&labels));
                    async move {
                        sync.fetch(&account, &id, &labels, Priority::Background)
                            .await
                    }
                },
                give_way,
            )
            .await?;
        out.through = ran.through;
        out.gave_way = ran.gave_way;
        Ok(out)
    }

    /// Download these messages now, at foreground priority: the person is
    /// waiting for them. Returns how many were stored whole.
    pub async fn download_now(
        &self,
        account: &EmailAddress,
        ids: Vec<ProviderMessageId>,
    ) -> SyncResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let labels = Arc::new(RwLock::new(self.refresh_labels(account).await?));
        let mut report = SyncReport::default();
        let account_for = account.clone();
        let quiet: ProgressFn = Arc::new(|_| {});
        self.for_each(
            ids.into_iter().map(|id| id.0).collect(),
            Phase::Bodies,
            &quiet,
            &mut report,
            move |sync, id| {
                let (account, labels) = (account_for.clone(), Arc::clone(&labels));
                async move {
                    sync.fetch(&account, &id, &labels, Priority::Foreground)
                        .await
                }
            },
        )
        .await?;
        Ok(report.fetched)
    }
}
