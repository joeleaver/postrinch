//! Delivering mail written here: messages to send and drafts to keep, in
//! the order they were made (see `postrinch-store`'s outgoing.rs).

use postrinch_model::EmailAddress;
use postrinch_store::{Outgoing, OutgoingKind};

use super::GmailSync;
use crate::{SyncError, SyncResult, build_message};

/// The `Message-ID` (without brackets) a queued message goes out with:
/// the same every attempt, so a send that may have gone through is found
/// rather than sent again.
pub(crate) fn outgoing_message_id(outgoing: &Outgoing) -> String {
    format!(
        "postrinch.{}.{}@{}",
        outgoing.created.as_millis(),
        outgoing.seq,
        outgoing.draft.account.domain()
    )
}

impl GmailSync {
    /// How much of the account's mail waits to be delivered.
    pub(super) async fn outgoing_waiting(&self, account: &EmailAddress) -> SyncResult<usize> {
        let account = account.clone();
        self.with_store(move |s| {
            Ok(s.outgoing_for(&account)?
                .iter()
                .filter(|o| !o.refused)
                .count())
        })
        .await
    }

    /// Deliver the account's waiting mail, oldest first. Mail Gmail refuses
    /// for good is marked so and waits for the person; any other failure
    /// stops here, for the next pass, and is returned. Returns how many
    /// went.
    pub async fn send_outgoing(&self, account: &EmailAddress) -> SyncResult<usize> {
        let owner = account.clone();
        let from_name = self
            .with_store(move |s| Ok(s.account(&owner)?.and_then(|a| a.display_name)))
            .await?;
        let owner = account.clone();
        let waiting = self.with_store(move |s| s.outgoing_for(&owner)).await?;
        let mut delivered = 0;
        for outgoing in waiting.into_iter().filter(|o| !o.refused) {
            let id = outgoing.id;
            match self.deliver(account, &outgoing, from_name.as_deref()).await {
                Ok(()) => {
                    self.with_store(move |s| s.outgoing_done(id)).await?;
                    delivered += 1;
                }
                Err(e @ SyncError::Api { status: 400, .. }) => {
                    tracing::warn!("{account}: Gmail refused outgoing mail: {e}");
                    let error = e.to_string();
                    self.with_store(move |s| s.outgoing_failed(id, &error, true))
                        .await?;
                }
                Err(e) => {
                    let error = e.to_string();
                    self.with_store(move |s| s.outgoing_failed(id, &error, false))
                        .await?;
                    return Err(e);
                }
            }
        }
        Ok(delivered)
    }

    async fn deliver(
        &self,
        account: &EmailAddress,
        outgoing: &Outgoing,
        from_name: Option<&str>,
    ) -> SyncResult<()> {
        let message_id = outgoing_message_id(outgoing);
        let raw = build_message(&outgoing.draft, from_name, &message_id);
        let thread = outgoing
            .draft
            .answering
            .thread
            .as_ref()
            .map(|t| t.0.as_str());
        let replaces = outgoing.draft.replaces.as_ref().map(|m| m.0.as_str());
        match outgoing.kind {
            OutgoingKind::Send => {
                // An attempt that failed may have reached Gmail first.
                let already = if outgoing.attempts > 0 {
                    self.api.find_by_message_id(account, &message_id).await?
                } else {
                    None
                };
                if already.is_none() {
                    self.api.send_message(account, &raw, thread).await?;
                }
                if let Some(draft) = replaces
                    && let Err(e) = self.api.delete_draft(account, draft).await
                {
                    tracing::warn!("{account}: the draft a sent message came from stays: {e}");
                }
            }
            OutgoingKind::Keep => {
                self.api.keep_draft(account, &raw, thread, replaces).await?;
            }
            OutgoingKind::Proposed => {}
        }
        Ok(())
    }
}
