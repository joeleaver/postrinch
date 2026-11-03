//! Full sync: list every message and make the store match the list.

use std::collections::HashSet;
use std::sync::Arc;

use postrinch_model::EmailAddress;
use tokio::sync::RwLock;

use super::{GmailSync, Progress, ProgressFn, SyncReport};
use crate::SyncResult;

impl GmailSync {
    /// List every message id in the account; fetch those the store lacks;
    /// remove stored ones Gmail no longer has. With `refresh_existing`, also
    /// bring the labels of messages already stored up to date (when the
    /// history that would have told us what changed is gone).
    pub(super) async fn full(
        &self,
        account: &EmailAddress,
        refresh_existing: bool,
        progress: &ProgressFn,
    ) -> SyncResult<SyncReport> {
        let mut report = SyncReport {
            full: true,
            ..Default::default()
        };
        let labels = Arc::new(RwLock::new(self.refresh_labels(account).await?));

        let mut listed: Vec<String> = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let list = self.api.list_messages(account, page.as_deref()).await?;
            listed.extend(list.messages.into_iter().map(|m| m.id));
            progress(Progress::Listing {
                found: listed.len(),
            });
            match list.next_page_token {
                Some(next) => page = Some(next),
                None => break,
            }
        }

        let a = account.clone();
        let stored: HashSet<String> = self
            .with_store(move |s| s.message_ids(&a))
            .await?
            .into_iter()
            .map(|id| id.0)
            .collect();
        let listed_set: HashSet<&String> = listed.iter().collect();

        // Newest first, as Gmail lists them, so the inbox fills from the top.
        let missing: Vec<String> = listed
            .iter()
            .filter(|id| !stored.contains(*id))
            .cloned()
            .collect();
        let account_for = account.clone();
        let fetch_labels = Arc::clone(&labels);
        self.for_each(missing, progress, &mut report, move |sync, id| {
            let (account, labels) = (account_for.clone(), Arc::clone(&fetch_labels));
            async move { sync.fetch(&account, &id, &labels).await }
        })
        .await?;

        if refresh_existing {
            let existing: Vec<String> = listed
                .iter()
                .filter(|id| stored.contains(*id))
                .cloned()
                .collect();
            let account_for = account.clone();
            let relabel_labels = Arc::clone(&labels);
            self.for_each(existing, progress, &mut report, move |sync, id| {
                let (account, labels) = (account_for.clone(), Arc::clone(&relabel_labels));
                async move { sync.relabel(&account, &id, &labels).await }
            })
            .await?;
        }

        for gone in stored.iter().filter(|id| !listed_set.contains(id)) {
            self.remove(account, gone).await?;
            report.removed += 1;
        }
        Ok(report)
    }
}
