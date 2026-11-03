//! The accounts, as Settings shows them.

use postrinch_model::Account;
use serde::{Deserialize, Serialize};

use crate::{CommandResult, Commands};

/// An account and how much of its mail is on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub account: Account,
    /// Messages stored, whole or by their headers alone.
    pub messages: usize,
    /// Of those, the ones whose bodies are downloaded.
    pub downloaded: usize,
}

impl Commands {
    /// Every account, in the order they were added.
    pub fn accounts(&self) -> CommandResult<Vec<AccountInfo>> {
        let mut infos = Vec::new();
        for account in self.store.accounts()? {
            let messages = self.store.message_ids(&account.address)?.len();
            let waiting = self.store.undownloaded(&account.address)?.len();
            infos.push(AccountInfo {
                account,
                messages,
                downloaded: messages.saturating_sub(waiting),
            });
        }
        Ok(infos)
    }
}
