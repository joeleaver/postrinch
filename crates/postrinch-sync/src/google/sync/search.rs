//! Asking Gmail to search: for mail whose bodies are not downloaded yet,
//! which the local index knows only by subject and people.

use std::sync::Arc;

use postrinch_model::{EmailAddress, ProviderMessageId};
use tokio::sync::RwLock;

use super::{GmailSync, Phase, ProgressFn, SyncReport};
use crate::SyncResult;

impl GmailSync {
    /// The ids of up to `max` messages Gmail finds for `query` (Gmail's
    /// search language), newest first. Any the store does not have yet
    /// are fetched by their headers and stored, so each can be shown.
    /// Costs 5 quota units, and 20 more for each message fetched.
    pub async fn search_remote(
        &self,
        account: &EmailAddress,
        query: &str,
        max: usize,
    ) -> SyncResult<Vec<ProviderMessageId>> {
        let ids = self.api.search(account, query, max).await?;
        let (a, check) = (account.clone(), ids.clone());
        let missing: Vec<String> = self
            .with_store(move |s| {
                let mut missing = Vec::new();
                for id in check {
                    if !s.has_message(&a, &ProviderMessageId(id.clone()))? {
                        missing.push(id);
                    }
                }
                Ok(missing)
            })
            .await?;
        if !missing.is_empty() {
            let labels = Arc::new(RwLock::new(self.refresh_labels(account).await?));
            let quiet: ProgressFn = Arc::new(|_| {});
            let mut report = SyncReport::default();
            let account_for = account.clone();
            self.for_each(
                missing,
                Phase::Headers,
                &quiet,
                &mut report,
                move |sync, id| {
                    let (account, labels) = (account_for.clone(), Arc::clone(&labels));
                    async move { sync.fetch_headers(&account, &id, &labels).await }
                },
            )
            .await?;
        }
        Ok(ids.into_iter().map(ProviderMessageId).collect())
    }
}
