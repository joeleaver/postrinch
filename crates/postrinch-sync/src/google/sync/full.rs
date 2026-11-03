//! Full sync: list every message and make the store match the list.

use std::collections::HashSet;
use std::sync::Arc;

use postrinch_model::EmailAddress;
use tokio::sync::RwLock;

use super::{GmailSync, Phase, Progress, ProgressFn, SyncReport};
use crate::SyncResult;
use crate::google::api::Priority;

/// How many of the newest messages a full sync downloads whole rather than
/// by their headers, so recent mail can be read at once.
pub(super) const WHOLE_FIRST: usize = 100;

/// Progress for one page's work, reported as part of the whole listing:
/// `before` messages were done on earlier pages, of `total` in all.
fn across_pages(progress: &ProgressFn, before: usize, total: usize) -> ProgressFn {
    let progress = Arc::clone(progress);
    Arc::new(move |p| match p {
        Progress::Working { phase, done, .. } => progress(Progress::Working {
            phase,
            done: before + done,
            total,
        }),
        other => progress(other),
    })
}

impl GmailSync {
    /// List every message id in the account; store the headers of those
    /// the store lacks (their bodies come later, from
    /// [`GmailSync::backfill`]); remove stored ones Gmail no longer has. With `refresh_existing`, also
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

        let a = account.clone();
        let stored: HashSet<String> = self
            .with_store(move |s| s.message_ids(&a))
            .await?
            .into_iter()
            .map(|id| id.0)
            .collect();

        // Gmail lists newest first. Each page is stored as it arrives, so
        // the inbox fills from the top while older pages are still being
        // listed: the newest messages whole, the rest by their headers
        // (a third of the cost; their bodies come later, newest first,
        // from backfill).
        // How many messages there are to fetch, for progress: the account's
        // count less those already here.
        let in_account = self.api.profile(account).await?.messages_total as usize;
        let mut listed: Vec<String> = Vec::new();
        let mut page: Option<String> = None;
        let mut done = 0;
        loop {
            let list = self.api.list_messages(account, page.as_deref()).await?;
            let first = listed.len();
            listed.extend(list.messages.into_iter().map(|m| m.id));
            progress(Progress::Listing {
                found: listed.len(),
            });
            let mut whole = Vec::new();
            let mut headers = Vec::new();
            for (i, id) in listed[first..].iter().enumerate() {
                if stored.contains(id) {
                    continue;
                }
                if first + i < self.whole_first {
                    whole.push(id.clone());
                } else {
                    headers.push(id.clone());
                }
            }
            let (whole_count, missing) = (whole.len(), whole.len() + headers.len());
            let total = in_account
                .max(listed.len())
                .saturating_sub(stored.len())
                .max(done + missing);
            let overall = across_pages(progress, done, total);
            let (account_for, whole_labels) = (account.clone(), Arc::clone(&labels));
            self.for_each(
                whole,
                Phase::Headers,
                &overall,
                &mut report,
                move |sync, id| {
                    let (account, labels) = (account_for.clone(), Arc::clone(&whole_labels));
                    async move {
                        sync.fetch(&account, &id, &labels, Priority::Foreground)
                            .await
                    }
                },
            )
            .await?;
            let overall = across_pages(progress, done + whole_count, total);
            let (account_for, header_labels) = (account.clone(), Arc::clone(&labels));
            self.for_each(
                headers,
                Phase::Headers,
                &overall,
                &mut report,
                move |sync, id| {
                    let (account, labels) = (account_for.clone(), Arc::clone(&header_labels));
                    async move { sync.fetch_headers(&account, &id, &labels).await }
                },
            )
            .await?;
            done += missing;
            match list.next_page_token {
                Some(next) => page = Some(next),
                None => break,
            }
        }
        let listed_set: HashSet<&String> = listed.iter().collect();

        if refresh_existing {
            let existing: Vec<String> = listed
                .iter()
                .filter(|id| stored.contains(*id))
                .cloned()
                .collect();
            let account_for = account.clone();
            let relabel_labels = Arc::clone(&labels);
            self.for_each(
                existing,
                Phase::Changes,
                progress,
                &mut report,
                move |sync, id| {
                    let (account, labels) = (account_for.clone(), Arc::clone(&relabel_labels));
                    async move { sync.relabel(&account, &id, &labels).await }
                },
            )
            .await?;
        }

        for gone in stored.iter().filter(|id| !listed_set.contains(id)) {
            self.remove(account, gone).await?;
            report.removed += 1;
        }
        Ok(report)
    }
}
