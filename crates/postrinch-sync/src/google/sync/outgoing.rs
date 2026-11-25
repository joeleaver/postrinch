//! Delivering mail written here: messages to send and drafts to keep, in
//! the order they were made (see `postrinch-store`'s outgoing.rs).

use postrinch_model::EmailAddress;
use postrinch_store::{Outgoing, OutgoingKind};

use super::GmailSync;
use crate::compose::outgoing_message_id;
use crate::{SyncError, SyncResult, build_message};

impl GmailSync {
    /// How much of the account's mail waits to be delivered.
    pub(crate) async fn outgoing_waiting(&self, account: &EmailAddress) -> SyncResult<usize> {
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
        let wanted = outgoing.draft.attachments.clone();
        let files = self
            .with_store(move |s| {
                wanted
                    .iter()
                    .map(|a| s.attachment_bytes(&a.blob).map(|b| (a.name.clone(), b)))
                    .collect::<Result<Vec<_>, _>>()
            })
            .await?;
        let mut bytes = Vec::with_capacity(files.len());
        for (name, file) in files {
            // Gone from this computer: the person must take it out or attach
            // it again, so this is a refusal, not a failure to retry.
            let file = file.ok_or_else(|| {
                SyncError::api(
                    "Postrinch",
                    400,
                    format!("{name} is no longer on this computer"),
                )
            })?;
            bytes.push(file);
        }
        let raw = build_message(&outgoing.draft, from_name, &message_id, &bytes);
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
                    // A minute's slack for clocks that disagree.
                    let since = outgoing.created.as_millis() / 1000 - 60;
                    self.api.find_sent(account, &message_id, since).await?
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
