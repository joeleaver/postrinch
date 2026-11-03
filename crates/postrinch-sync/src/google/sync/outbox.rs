//! Sending the outbox: changes made here, to Gmail.
//!
//! The store applies a change at once and keeps it as an outbox entry (see
//! `postrinch-store`'s outbox.rs). This sends what waits, oldest first, and
//! marks each entry sent; a sync that begins after an entry was sent and
//! finishes forgets it ([`GmailSync::sync`]).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use postrinch_model::{EmailAddress, Timestamp};
use postrinch_store::OutboxEntry;
use tokio::sync::RwLock;

use super::GmailSync;
use crate::google::labels::{STARRED, TRASH, UNREAD};
use crate::{SyncError, SyncResult};

/// `messages.batchModify` takes at most this many messages a call.
const BATCH: usize = 1000;

/// What sending an account's outbox did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SendReport {
    pub sent: usize,
    /// Why Gmail refused the changes it would never take. Each was dropped
    /// and its messages read again from Gmail.
    pub refused: Vec<String>,
}

/// Now, as the store keeps times.
pub(super) fn now() -> Timestamp {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Timestamp::from_millis(millis)
}

/// Whether Gmail refused a change for good: a malformed request, or a
/// message or label it does not have. Anything else (offline, a rate limit
/// that outlasted the retries, Gmail unwell) may pass.
fn refused_for_good(e: &SyncError) -> bool {
    matches!(
        e,
        SyncError::Api {
            status: 400 | 404,
            ..
        }
    )
}

impl GmailSync {
    /// How many of the account's changes wait to be sent.
    pub async fn waiting(&self, account: &EmailAddress) -> SyncResult<usize> {
        let account = account.clone();
        self.with_store(move |s| {
            Ok(s.outbox(&account)?
                .iter()
                .filter(|e| e.sent.is_none())
                .count())
        })
        .await
    }

    /// Send the account's waiting changes, oldest first. One that fails
    /// for a reason that may pass stays to be tried again, and the ones
    /// after it wait behind it, so Gmail gets them in the order they were
    /// made; the error is returned. One Gmail refuses for good is dropped,
    /// and the messages it named are read again so what is shown matches
    /// Gmail.
    pub async fn send_outbox(&self, account: &EmailAddress) -> SyncResult<SendReport> {
        let mut report = SendReport::default();
        let owner = account.clone();
        let entries = self.with_store(move |s| s.outbox(&owner)).await?;
        for entry in entries.into_iter().filter(|e| e.sent.is_none()) {
            let id = entry.id;
            match self.send_change(account, &entry).await {
                Ok(()) => {
                    self.with_store(move |s| s.outbox_sent(id)).await?;
                    report.sent += 1;
                }
                Err(e) if refused_for_good(&e) => {
                    tracing::warn!("{account}: Gmail refused a change: {e}");
                    self.with_store(move |s| s.outbox_drop(id)).await?;
                    self.read_again(account, &entry).await?;
                    report.refused.push(e.to_string());
                }
                Err(e) => {
                    let error = e.to_string();
                    self.with_store(move |s| s.outbox_failed(id, &error))
                        .await?;
                    return Err(e);
                }
            }
        }
        Ok(report)
    }

    /// One entry, as Gmail's calls: the trash first, then labels and flags.
    async fn send_change(&self, account: &EmailAddress, entry: &OutboxEntry) -> SyncResult<()> {
        let ids: Vec<String> = entry.messages.iter().map(|m| m.0.clone()).collect();
        let change = &entry.change;
        if change.add_labels.iter().any(|l| l == TRASH) {
            for id in &ids {
                self.api.trash(account, id).await?;
            }
        }
        if change.remove_labels.iter().any(|l| l == TRASH) {
            for id in &ids {
                self.api.untrash(account, id).await?;
            }
        }
        let mut add: Vec<String> = change
            .add_labels
            .iter()
            .filter(|l| *l != TRASH)
            .cloned()
            .collect();
        let mut remove: Vec<String> = change
            .remove_labels
            .iter()
            .filter(|l| *l != TRASH)
            .cloned()
            .collect();
        for (flag, label) in [(change.unread, UNREAD), (change.starred, STARRED)] {
            match flag {
                Some(true) => add.push(label.to_owned()),
                Some(false) => remove.push(label.to_owned()),
                None => {}
            }
        }
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
        for chunk in ids.chunks(BATCH) {
            self.api.batch_modify(account, chunk, &add, &remove).await?;
        }
        Ok(())
    }

    /// Read an entry's messages again from Gmail, after it was dropped.
    async fn read_again(&self, account: &EmailAddress, entry: &OutboxEntry) -> SyncResult<()> {
        let labels = Arc::new(RwLock::new(self.refresh_labels(account).await?));
        for message in &entry.messages {
            self.relabel(account, &message.0, &labels).await?;
        }
        Ok(())
    }
}
