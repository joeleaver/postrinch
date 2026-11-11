//! Mail one message at a time, for a person who reads it that way rather
//! than as conversations: a folder's messages, newest first, each a row of
//! its own, and actions that change only the messages they name.

use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, Message, ProviderMessageId, SystemLabel, ThreadSummary};
use serde::{Deserialize, Serialize};

use crate::labels::Chip;
use crate::search::excerpt;
use crate::{CommandResult, Commands, Folder, ThreadAction, ThreadPage, ThreadRow};

/// Whether mail is listed as conversations or as messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    /// One row per thread.
    #[default]
    Conversations,
    /// One row per message.
    Messages,
}

/// A message, as an action names it.
pub type MessageRef = (EmailAddress, ProviderMessageId);

impl Commands {
    /// The folder's messages from `offset`, at most `limit` of them, newest
    /// first, each a row of its own ([`ThreadRow::message`] names it). With
    /// a `category`, only messages Gmail also filed under it.
    pub fn list_messages(
        &self,
        folder: &Folder,
        category: Option<SystemLabel>,
        offset: usize,
        limit: usize,
    ) -> CommandResult<ThreadPage> {
        let mut entries = Vec::new();
        let mut account_chips: HashMap<String, HashMap<String, Chip>> = HashMap::new();
        for (account, label) in self.sources(folder)? {
            let mut found = self.store.label_messages(&account, &label)?;
            if let Some(category) = category {
                let in_category: HashSet<ProviderMessageId> =
                    match self.system_label_id(&account, category)? {
                        Some(id) => self
                            .store
                            .label_messages(&account, &id)?
                            .into_iter()
                            .map(|e| e.message)
                            .collect(),
                        None => HashSet::new(),
                    };
                found.retain(|e| in_category.contains(&e.message));
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
            b.received
                .cmp(&a.received)
                .then_with(|| a.account.key().cmp(&b.account.key()))
                .then_with(|| a.message.cmp(&b.message))
        });
        let total = entries.len();
        let mut threads = Vec::new();
        for entry in entries.iter().skip(offset).take(limit) {
            let Some(message) = self.store.message(&entry.account, &entry.message)? else {
                continue;
            };
            let known = &account_chips[&entry.account.key()];
            if let Some(row) = message_row(&message, known, listed, None) {
                threads.push(row);
            }
        }
        Ok(ThreadPage {
            total,
            offset,
            threads,
        })
    }

    /// Do `action` to each message alone, as one message at a time does:
    /// archiving one leaves the rest of its conversation in the inbox, and
    /// starring or marking one unread changes that one. Messages the store
    /// does not have are passed over.
    pub fn act_messages(
        &self,
        messages: &[MessageRef],
        action: &ThreadAction,
    ) -> CommandResult<()> {
        for (account, _) in messages {
            self.check(account)?;
        }
        for (account, id) in messages {
            let Some(message) = self.store.message(account, id)? else {
                continue;
            };
            for (ids, change) in self.changes(account, std::slice::from_ref(&message), action)? {
                self.store.change_messages(account, &ids, &change)?;
            }
        }
        Ok(())
    }
}

/// A message as its own row: its summary as a one-message thread, the
/// tag-like labels it carries but the one listed, and where `terms` matched
/// it when it was searched for.
pub(crate) fn message_row(
    message: &Message,
    known: &HashMap<String, Chip>,
    listed: Option<&str>,
    terms: Option<&[String]>,
) -> Option<ThreadRow> {
    let summary = ThreadSummary::from_messages(std::iter::once(message))?;
    let chips = summary
        .labels
        .iter()
        .filter(|l| Some(l.as_str()) != listed)
        .filter_map(|l| known.get(l).cloned())
        .collect();
    let excerpt = terms.and_then(|terms| {
        let text = format!("{}\n{}", message.subject(), message.body_text);
        excerpt(&text, terms)
    });
    Some(ThreadRow {
        summary,
        chips,
        excerpt,
        remote: false,
        message: Some(message.id.clone()),
    })
}
