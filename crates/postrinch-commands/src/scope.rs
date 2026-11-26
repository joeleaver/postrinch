//! Which accounts a caller sees. The window sees every account; an agent
//! sees those the person gave it, and to it the others do not exist. Mail
//! accounts and social accounts are given apart: an agent given a mail
//! account sees no social account by the same address.

use std::collections::HashSet;
use std::sync::Arc;

use postrinch_model::{Account, EmailAddress, Provider, SavedView};
use postrinch_store::social_account_key;

use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// The same commands, seeing only those of the mail `accounts` they
    /// already see, and no social account: limiting never widens. Asking
    /// about any other account answers as for one the store does not have.
    pub fn limited_to(&self, accounts: &[EmailAddress]) -> Self {
        self.limited(accounts, &[])
    }

    /// The same commands, seeing only those of the mail `accounts` and the
    /// social accounts `actors` (in any spelling) they already see.
    pub fn limited(&self, accounts: &[EmailAddress], actors: &[&str]) -> Self {
        let reach = Reach {
            mail: accounts
                .iter()
                .filter(|a| self.sees(a))
                .map(EmailAddress::key)
                .collect(),
            social: actors
                .iter()
                .filter(|a| self.sees_social(a))
                .map(|a| social_account_key(a))
                .collect(),
        };
        Self {
            store: self.store.clone(),
            only: Some(Arc::new(reach)),
        }
    }

    /// Whether this caller sees the mail `account`.
    pub fn sees(&self, account: &EmailAddress) -> bool {
        self.only
            .as_ref()
            .is_none_or(|only| only.mail.contains(&account.key()))
    }

    /// Whether this caller sees the social account of `actor`, in any
    /// spelling.
    pub fn sees_social(&self, actor: &str) -> bool {
        self.only
            .as_ref()
            .is_none_or(|only| only.social.contains(&social_account_key(actor)))
    }

    /// Whether this caller sees every account, as the window does.
    pub(crate) fn sees_all(&self) -> bool {
        self.only.is_none()
    }

    /// An error for an account this caller does not see.
    pub fn check(&self, account: &EmailAddress) -> CommandResult<()> {
        if self.sees(account) {
            Ok(())
        } else {
            Err(CommandError::UnknownAccount(account.clone()))
        }
    }

    /// The account, when this caller sees it and the store has it.
    pub fn account(&self, account: &EmailAddress) -> CommandResult<Account> {
        self.check(account)?;
        self.store
            .account(account)?
            .ok_or_else(|| CommandError::UnknownAccount(account.clone()))
    }

    /// An error for an account with no calendars: calendars are Google
    /// Calendar's, so an IMAP account has none to read, change or answer
    /// invitations in.
    pub fn check_calendars(&self, account: &EmailAddress) -> CommandResult<()> {
        match self.account(account)?.provider {
            Provider::Google => Ok(()),
            Provider::Imap => Err(CommandError::Invalid(format!(
                "calendars are Google accounts' only, and {account} is an IMAP account"
            ))),
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

/// The accounts a caller is limited to, by key.
pub(crate) struct Reach {
    mail: HashSet<String>,
    /// Social accounts, by [`social_account_key`].
    social: HashSet<String>,
}

/// What a caller is limited to; `None` for every account.
pub(crate) type Only = Option<Arc<Reach>>;
