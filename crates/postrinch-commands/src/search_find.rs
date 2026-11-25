//! Running a search: each alternative of a query (query_or.rs) turned into
//! the store's [`Find`] over the accounts it looks in, its labels found by
//! name in each account, and the messages found, each once.

use std::collections::HashSet;

use postrinch_model::{EmailAddress, LabelKind};
use postrinch_store::{Find, Found};

use crate::query::{LabelName, Last, Query, label_matches};
use crate::query_or::Either;
use crate::system_labels::system_label_in;
use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// Every message a query's alternatives match, newest first, each
    /// once, and the accounts it looked in. `every`: not only the most
    /// relevant word matches ([`Find::every`]).
    pub(crate) fn found(
        &self,
        query: &Either,
        every: bool,
    ) -> CommandResult<(Vec<Found>, Vec<EmailAddress>)> {
        self.found_in(query, every, &[])
    }

    /// [`Commands::found`] in only these accounts (every account for none).
    pub(crate) fn found_in(
        &self,
        query: &Either,
        every: bool,
        only: &[EmailAddress],
    ) -> CommandResult<(Vec<Found>, Vec<EmailAddress>)> {
        let mut accounts: Vec<EmailAddress> = Vec::new();
        let mut found: Vec<Found> = Vec::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for one in &query.queries {
            let mut these = self.search_accounts(one)?;
            if !only.is_empty() {
                these.retain(|a| only.contains(a));
                // The store reads no accounts as every account.
                if these.is_empty() {
                    continue;
                }
            }
            let mut find = self.to_find(one, &these)?;
            find.every = every;
            for f in self.store.find_messages(&find)? {
                if seen.insert((f.account.key(), f.id.0.clone())) {
                    found.push(f);
                }
            }
            for a in these {
                if !accounts.contains(&a) {
                    accounts.push(a);
                }
            }
        }
        if query.queries.len() > 1 {
            found.sort_by(|a, b| b.received.cmp(&a.received).then_with(|| a.id.cmp(&b.id)));
        }
        if let Some(last) = query.last()? {
            found = self.store.whose_last(found, last == Last::Me)?;
        }
        Ok((found, accounts))
    }

    /// The accounts a query looks in: those this caller sees, narrowed by
    /// `account:`.
    pub(crate) fn search_accounts(&self, query: &Query) -> CommandResult<Vec<EmailAddress>> {
        let mut accounts: Vec<EmailAddress> = self
            .visible_accounts()?
            .into_iter()
            .map(|a| a.address)
            .collect();
        for part in &query.accounts {
            let part = part.to_lowercase();
            accounts.retain(|a| a.key().contains(&part));
            if accounts.is_empty() {
                return Err(CommandError::Invalid(format!(
                    "No account matches account:{part}."
                )));
            }
        }
        Ok(accounts)
    }

    /// The store's form of a query over `accounts`, labels found by name.
    pub(crate) fn to_find(&self, query: &Query, accounts: &[EmailAddress]) -> CommandResult<Find> {
        let resolve = |names: &[LabelName]| -> CommandResult<Vec<Vec<(EmailAddress, String)>>> {
            let mut all = Vec::new();
            for name in names {
                let mut any = Vec::new();
                for account in accounts {
                    let labels = self.store.labels(account)?;
                    let ids = match name {
                        LabelName::System(s) => system_label_in(&labels, *s)
                            .map(|l| l.provider_id.clone())
                            .into_iter()
                            .collect(),
                        LabelName::Named(n) => labels
                            .iter()
                            .filter(|l| l.kind == LabelKind::User && label_matches(&l.name, n))
                            .map(|l| l.provider_id.clone())
                            .collect::<Vec<_>>(),
                    };
                    any.extend(ids.into_iter().map(|id| (account.clone(), id)));
                }
                if any.is_empty()
                    && let LabelName::Named(n) = name
                {
                    return Err(CommandError::Invalid(format!("No label is called {n}.")));
                }
                all.push(any);
            }
            Ok(all)
        };
        Ok(Find {
            text: query.text(),
            text_without: query.text_without(),
            accounts: accounts.to_vec(),
            from: query.from.clone(),
            to: query.to.clone(),
            not_from: query.not_from.clone(),
            not_to: query.not_to.clone(),
            subject: query.subject.clone(),
            labels: resolve(&query.labels)?,
            without: resolve(&query.without)?,
            unread: query.unread,
            starred: query.starred,
            attachments: query.attachments,
            after: query.after,
            before: query.before,
            larger: query.larger,
            smaller: query.smaller,
            within: None,
            set_aside_too: query.set_aside_too,
            every: false,
        })
    }
}
