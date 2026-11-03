//! Which accounts a caller sees. The window sees every account; an agent
//! sees those the person gave it, and to it the others do not exist.

use std::collections::HashSet;
use std::sync::Arc;

use postrinch_model::{Account, EmailAddress};

use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// The same commands, seeing only `accounts`. Asking about any other
    /// account answers as for one the store does not have.
    pub fn limited_to(&self, accounts: &[EmailAddress]) -> Self {
        Self {
            store: self.store.clone(),
            only: Some(Arc::new(accounts.iter().map(EmailAddress::key).collect())),
        }
    }

    /// Whether this caller sees `account`.
    pub fn sees(&self, account: &EmailAddress) -> bool {
        self.only
            .as_ref()
            .is_none_or(|only| only.contains(&account.key()))
    }

    /// An error for an account this caller does not see.
    pub(crate) fn check(&self, account: &EmailAddress) -> CommandResult<()> {
        if self.sees(account) {
            Ok(())
        } else {
            Err(CommandError::UnknownAccount(account.clone()))
        }
    }

    /// The accounts this caller sees, in the order they were added.
    pub(crate) fn visible_accounts(&self) -> CommandResult<Vec<Account>> {
        let mut accounts = self.store.accounts()?;
        accounts.retain(|a| self.sees(&a.address));
        Ok(accounts)
    }
}

/// Account keys a caller is limited to.
pub(crate) type Only = Option<Arc<HashSet<String>>>;
