//! The quick way to list a view that only names a label or two: through
//! the labels' links rather than a search of every message.

use std::collections::HashMap;

use postrinch_model::{EmailAddress, LabelKind};
use postrinch_store::ThreadEntry;

use crate::query::{LabelName, Query, label_matches};
use crate::query_or::Either;
use crate::system_labels::system_label_in;
use crate::{CommandResult, Commands};

impl Commands {
    /// The conversations a search finds when it only names a label or two
    /// (`in:inbox category:primary`, the tabs and most views), read through
    /// the labels' links alone, which is many times quicker than reading
    /// every message the labels have. `None` for any other search.
    pub(crate) fn labelled_threads(
        &self,
        either: &Either,
        only: &[EmailAddress],
    ) -> CommandResult<Option<Vec<ThreadEntry>>> {
        let labels_only = |q: &Query| {
            (1..=2).contains(&q.labels.len())
                && Query {
                    labels: Vec::new(),
                    accounts: Vec::new(),
                    gmail: String::new(),
                    ..q.clone()
                } == Query::default()
        };
        if !either.queries.iter().all(labels_only) {
            return Ok(None);
        }
        let mut threads: HashMap<(String, String), ThreadEntry> = HashMap::new();
        for q in &either.queries {
            for account in self.search_accounts(q)? {
                if !only.is_empty() && !only.contains(&account) {
                    continue;
                }
                let labels = self.store.labels(&account)?;
                let id = |name: &LabelName| {
                    match name {
                        LabelName::System(s) => system_label_in(&labels, *s),
                        LabelName::Named(n) => labels
                            .iter()
                            .find(|l| l.kind == LabelKind::User && label_matches(&l.name, n)),
                    }
                    .map(|l| l.provider_id.clone())
                };
                let ids: Option<Vec<String>> = q.labels.iter().map(id).collect();
                let Some(ids) = ids else { continue };
                // In an account without categories, `in:inbox
                // category:primary` names the inbox twice.
                let within = ids.get(1).filter(|id| **id != ids[0]).map(String::as_str);
                for t in self
                    .store
                    .label_threads_sorting(&account, &ids[0], within, true)?
                {
                    let key = (t.account.key(), t.thread.0.clone());
                    let unread = t.unread;
                    threads
                        .entry(key)
                        .and_modify(|had| had.unread |= unread)
                        .or_insert(t);
                }
            }
        }
        let mut threads: Vec<ThreadEntry> = threads.into_values().collect();
        threads.sort_by(|a, b| {
            b.latest
                .cmp(&a.latest)
                .then_with(|| a.thread.cmp(&b.thread))
        });
        Ok(Some(threads))
    }
}
