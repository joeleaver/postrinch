//! Making, renaming and deleting labels in Gmail itself, for agents and
//! the window alike. Each call runs on sync's runtime and waits for it, so
//! it is made from a thread of its own, never the window's.

use postrinch_agents::Gmail;
use postrinch_model::EmailAddress;

use crate::sync::SyncHandle;

pub struct GmailLabels(pub SyncHandle);

impl GmailLabels {
    fn run<T>(
        &self,
        work: impl std::future::Future<Output = postrinch_sync::SyncResult<T>>,
    ) -> Result<T, String> {
        self.0.runtime.block_on(work).map_err(|e| {
            tracing::warn!("a label change in Gmail failed: {e}");
            match e {
                postrinch_sync::SyncError::NeedsSignIn(_) => {
                    "That account must sign in again in Postrinch first.".to_owned()
                }
                postrinch_sync::SyncError::Http(_) => {
                    "Gmail could not be reached; try again when online.".to_owned()
                }
                other => format!("Gmail refused: {other}."),
            }
        })
    }
}

impl Gmail for GmailLabels {
    fn create_label(&self, account: &EmailAddress, name: &str) -> Result<String, String> {
        let sync = self.0.sync_engine();
        Ok(self.run(sync.create_label(account, name))?.provider_id)
    }

    fn rename_label(&self, account: &EmailAddress, id: &str, name: &str) -> Result<(), String> {
        let sync = self.0.sync_engine();
        self.run(sync.rename_label(account, id, name)).map(|_| ())
    }

    fn delete_label(&self, account: &EmailAddress, id: &str) -> Result<(), String> {
        let sync = self.0.sync_engine();
        self.run(sync.delete_label(account, id))
    }
}
