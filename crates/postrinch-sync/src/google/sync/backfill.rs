//! Backfill: downloading the bodies of messages known only by their headers.

use std::sync::Arc;

use postrinch_model::EmailAddress;
use tokio::sync::RwLock;

use super::{GmailSync, Phase, ProgressFn, SyncReport};
use crate::SyncResult;
use crate::google::api::Priority;

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
        let mut report = SyncReport::default();
        let a = account.clone();
        let ids: Vec<String> = self
            .with_store(move |s| s.undownloaded(&a))
            .await?
            .into_iter()
            .map(|id| id.0)
            .collect();
        if ids.is_empty() {
            return Ok(report);
        }
        let labels = Arc::new(RwLock::new(self.refresh_labels(account).await?));
        let account_for = account.clone();
        self.for_each(
            ids,
            Phase::Bodies,
            &progress,
            &mut report,
            move |sync, id| {
                let (account, labels) = (account_for.clone(), Arc::clone(&labels));
                async move {
                    sync.fetch(&account, &id, &labels, Priority::Background)
                        .await
                }
            },
        )
        .await?;
        Ok(report)
    }
}
