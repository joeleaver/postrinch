//! The person's labels, made, renamed and deleted in Gmail at once and
//! then kept here, so what is asked next can use them.

use postrinch_model::{EmailAddress, Label};

use super::GmailSync;
use crate::google::api::LabelResource;
use crate::google::labels::to_label;
use crate::{SyncError, SyncResult};

impl GmailSync {
    /// Make a label in the account; returns it as stored.
    pub async fn create_label(&self, account: &EmailAddress, name: &str) -> SyncResult<Label> {
        let made = self.api.create_label(account, name).await?;
        self.keep(account, &made)
    }

    /// Rename one of the account's labels; its mail keeps it.
    pub async fn rename_label(
        &self,
        account: &EmailAddress,
        id: &str,
        name: &str,
    ) -> SyncResult<Label> {
        // What the person chose for it here (pinned, hidden) stays: those
        // are kept apart from the label itself.
        let renamed = self.api.rename_label(account, id, name).await?;
        self.keep(account, &renamed)
    }

    /// Delete one of the account's labels. Its mail stays, without it.
    pub async fn delete_label(&self, account: &EmailAddress, id: &str) -> SyncResult<()> {
        self.api.delete_label(account, id).await?;
        self.store.remove_label(account, id)?;
        Ok(())
    }

    fn keep(&self, account: &EmailAddress, resource: &LabelResource) -> SyncResult<Label> {
        let label = to_label(resource).ok_or_else(|| {
            SyncError::api("Gmail", 200, "it answered with a label it does not make")
        })?;
        self.store.put_label(account, &label)?;
        Ok(label)
    }
}
