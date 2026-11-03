//! Incremental sync: apply Gmail's history since the cursor.

use std::collections::BTreeSet;
use std::sync::Arc;

use postrinch_model::{EmailAddress, ProviderMessageId};
use tokio::sync::RwLock;

use super::cursor::Cursor;
use super::{GmailSync, Phase, ProgressFn, SyncReport};
use crate::SyncResult;
use crate::google::api::Priority;

impl GmailSync {
    /// Read history from `since` to now and apply it: remove deleted
    /// messages, fetch new ones, update the labels of changed ones. Then
    /// move the cursor to where history ended. Fails with a 404
    /// ([`crate::SyncError::is_not_found`]) when `since` has expired.
    pub(super) async fn incremental(
        &self,
        account: &EmailAddress,
        since: &str,
        progress: &ProgressFn,
    ) -> SyncResult<SyncReport> {
        let mut report = SyncReport::default();
        let mut deleted = BTreeSet::new();
        let mut touched = BTreeSet::new();
        let mut page: Option<String> = None;
        let latest = loop {
            let history = self.api.history(account, since, page.as_deref()).await?;
            for record in history.history {
                deleted.extend(record.messages_deleted.into_iter().map(|m| m.message.id));
                touched.extend(
                    record
                        .messages_added
                        .into_iter()
                        .chain(record.labels_added)
                        .chain(record.labels_removed)
                        .map(|m| m.message.id),
                );
            }
            match history.next_page_token {
                Some(next) => page = Some(next),
                None => break history.history_id,
            }
        };

        for id in &deleted {
            self.remove(account, id).await?;
            report.removed += 1;
        }

        let changed: Vec<String> = touched.difference(&deleted).cloned().collect();
        if !changed.is_empty() {
            let labels = Arc::new(RwLock::new(self.refresh_labels(account).await?));
            let account_for = account.clone();
            self.for_each(
                changed,
                Phase::Changes,
                progress,
                &mut report,
                move |sync, id| {
                    let (account, labels) = (account_for.clone(), Arc::clone(&labels));
                    async move {
                        let a = account.clone();
                        let mid = ProviderMessageId(id.clone());
                        if sync.with_store(move |s| s.has_message(&a, &mid)).await? {
                            sync.relabel(&account, &id, &labels).await
                        } else {
                            // New mail is what the person is waiting for:
                            // download it whole, now.
                            sync.fetch(&account, &id, &labels, Priority::Foreground)
                                .await
                        }
                    }
                },
            )
            .await?;
        }

        self.set_cursor(account, Cursor::History { history_id: latest })
            .await?;
        Ok(report)
    }
}
