//! Which accounts a caller sees. The window sees every account; an agent
//! sees those the person gave it, and to it the others do not exist.

use std::collections::HashSet;
use std::sync::Arc;

use postrinch_model::{Account, EmailAddress, SavedView};

use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// The same commands, seeing only those of `accounts` they already
    /// see: limiting never widens. Asking about any other account answers
    /// as for one the store does not have.
    pub fn limited_to(&self, accounts: &[EmailAddress]) -> Self {
        let only = accounts
            .iter()
            .filter(|a| self.sees(a))
            .map(EmailAddress::key)
            .collect();
        Self {
            store: self.store.clone(),
            only: Some(Arc::new(only)),
        }
    }

    /// Whether this caller sees `account`.
    pub fn sees(&self, account: &EmailAddress) -> bool {
        self.only
            .as_ref()
            .is_none_or(|only| only.contains(&account.key()))
    }

    /// An error for an account this caller does not see.
    pub fn check(&self, account: &EmailAddress) -> CommandResult<()> {
        if self.sees(account) {
            Ok(())
        } else {
            Err(CommandError::UnknownAccount(account.clone()))
        }
    }

    /// Whether this caller sees `view`: one of every account, or of
    /// accounts it sees each of. A view of some account it does not see
    /// is not there for it.
    pub(crate) fn sees_view(&self, view: &SavedView) -> bool {
        view.accounts.iter().all(|a| self.sees(a))
    }

    /// Whether this caller may make or change `view`. The window may
    /// change any; an agent limited to some accounts only views of those
    /// accounts alone, as a view of every account shows the person mail
    /// the agent does not see.
    pub fn may_change_view(&self, view: &SavedView) -> bool {
        self.only.is_none() || (!view.accounts.is_empty() && self.sees_view(view))
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
