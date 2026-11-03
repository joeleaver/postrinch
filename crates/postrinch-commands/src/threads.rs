//! Listing a folder's threads and reading one.

use std::collections::{HashMap, HashSet};

use postrinch_model::{
    EmailAddress, LabelKind, Message, ProviderThreadId, SystemLabel, ThreadSummary,
};
use serde::{Deserialize, Serialize};

use crate::labels::Chip;
use crate::search::Excerpt;
use crate::{CommandResult, Commands, Folder};

/// A page of a folder's threads, latest activity first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadPage {
    /// How many threads the folder has in all.
    pub total: usize,
    pub offset: usize,
    pub threads: Vec<ThreadRow>,
}

/// A thread as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRow {
    pub summary: ThreadSummary,
    /// Its tag-like labels, leaving out the one being listed.
    pub chips: Vec<Chip>,
    /// Where a search matched it; `None` in a folder listing.
    pub excerpt: Option<Excerpt>,
}

/// Gmail's inbox categories, in the order its tabs show them.
pub const CATEGORIES: [SystemLabel; 5] = [
    SystemLabel::CategoryPersonal,
    SystemLabel::CategoryUpdates,
    SystemLabel::CategoryPromotions,
    SystemLabel::CategorySocial,
    SystemLabel::CategoryForums,
];

/// A thread with all its messages, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub summary: ThreadSummary,
    pub messages: Vec<Message>,
}

impl Commands {
    /// The folder's threads from `offset`, at most `limit` of them. With a
    /// `category` (one of [`CATEGORIES`]), only threads Gmail also filed
    /// under it, as its inbox tabs do.
    pub fn list_threads(
        &self,
        folder: &Folder,
        category: Option<SystemLabel>,
        offset: usize,
        limit: usize,
    ) -> CommandResult<ThreadPage> {
        let mut entries = Vec::new();
        let mut account_chips: HashMap<String, HashMap<String, Chip>> = HashMap::new();
        for (account, label) in self.sources(folder)? {
            let mut found = self.store.label_threads(&account, &label)?;
            if let Some(category) = category {
                let in_category: HashSet<ProviderThreadId> =
                    match self.system_label_id(&account, category)? {
                        Some(id) => self
                            .store
                            .label_threads(&account, &id)?
                            .into_iter()
                            .map(|e| e.thread)
                            .collect(),
                        None => HashSet::new(),
                    };
                found.retain(|e| in_category.contains(&e.thread));
            }
            entries.extend(found);
            if let std::collections::hash_map::Entry::Vacant(slot) =
                account_chips.entry(account.key())
            {
                slot.insert(self.account_chips(&account)?);
            }
        }
        let listed = match folder {
            Folder::Label { label, .. } => Some(label.as_str()),
            Folder::Everywhere(_) => None,
        };
        entries.sort_by(|a, b| {
            b.latest
                .cmp(&a.latest)
                .then_with(|| a.account.key().cmp(&b.account.key()))
                .then_with(|| a.thread.cmp(&b.thread))
        });
        let total = entries.len();
        let mut threads = Vec::new();
        for entry in entries.iter().skip(offset).take(limit) {
            let messages = self.store.thread_messages(&entry.account, &entry.thread)?;
            let Some(summary) = ThreadSummary::from_messages(&messages) else {
                continue;
            };
            let known = &account_chips[&entry.account.key()];
            let chips = summary
                .labels
                .iter()
                .filter(|l| Some(l.as_str()) != listed)
                .filter_map(|l| known.get(l).cloned())
                .collect();
            threads.push(ThreadRow {
                summary,
                chips,
                excerpt: None,
            });
        }
        Ok(ThreadPage {
            total,
            offset,
            threads,
        })
    }

    /// The provider's id for one of an account's system labels.
    fn system_label_id(
        &self,
        account: &EmailAddress,
        system: SystemLabel,
    ) -> CommandResult<Option<String>> {
        Ok(self
            .store
            .labels(account)?
            .into_iter()
            .find(|l| l.kind == LabelKind::System(system))
            .map(|l| l.provider_id))
    }

    /// A thread and its messages. `None` for a thread the store does not
    /// have.
    pub fn read_thread(
        &self,
        account: &EmailAddress,
        thread: &ProviderThreadId,
    ) -> CommandResult<Option<Thread>> {
        self.check(account)?;
        let messages = self.store.thread_messages(account, thread)?;
        Ok(ThreadSummary::from_messages(&messages).map(|summary| Thread { summary, messages }))
    }
}
