//! The outbox, held in memory beside the database, so that taking the next
//! sequence number and applying waiting changes over a message's labels
//! don't read every entry. With thousands of changes waiting, each new
//! change and each message sync writes read them all.
//!
//! It is read from the database once, when first needed, and every write
//! of an entry goes through [`crate::Store`]'s outbox methods, which update
//! it after the database. The store is opened by one process at a time, so
//! nothing else writes entries behind it.

use std::collections::{BTreeMap, HashMap};

use postrinch_model::ProviderMessageId;

use crate::OutboxEntry;

#[derive(Default)]
pub(crate) struct OutboxCache {
    /// By the entry's object id.
    entries: BTreeMap<u64, OutboxEntry>,
    /// The entries naming each message, by account key and message id.
    by_message: HashMap<(String, String), Vec<u64>>,
    /// The highest sequence number given out so far.
    max_seq: u64,
}

impl OutboxCache {
    pub(crate) fn new(all: Vec<OutboxEntry>) -> Self {
        let mut cache = Self::default();
        for entry in all {
            cache.insert(entry);
        }
        cache
    }

    pub(crate) fn insert(&mut self, entry: OutboxEntry) {
        self.max_seq = self.max_seq.max(entry.seq);
        let account = entry.account.key();
        for m in &entry.messages {
            self.by_message
                .entry((account.clone(), m.0.clone()))
                .or_default()
                .push(entry.id);
        }
        self.entries.insert(entry.id, entry);
    }

    pub(crate) fn remove(&mut self, id: u64) {
        let Some(entry) = self.entries.remove(&id) else {
            return;
        };
        let account = entry.account.key();
        for m in &entry.messages {
            let key = (account.clone(), m.0.clone());
            if let Some(ids) = self.by_message.get_mut(&key) {
                ids.retain(|i| *i != id);
                if ids.is_empty() {
                    self.by_message.remove(&key);
                }
            }
        }
    }

    pub(crate) fn update(&mut self, id: u64, change: impl FnOnce(&mut OutboxEntry)) {
        if let Some(entry) = self.entries.get_mut(&id) {
            change(entry);
        }
    }

    pub(crate) fn get(&self, id: u64) -> Option<&OutboxEntry> {
        self.entries.get(&id)
    }

    pub(crate) fn max_seq(&self) -> u64 {
        self.max_seq
    }

    /// The account's entries in the order they were made.
    pub(crate) fn of_account(&self, account_key: &str) -> Vec<OutboxEntry> {
        let mut found: Vec<OutboxEntry> = self
            .entries
            .values()
            .filter(|e| e.account.key() == account_key)
            .cloned()
            .collect();
        found.sort_by_key(|e| e.seq);
        found
    }

    /// The entries naming one message, in the order they were made.
    pub(crate) fn of_message(
        &self,
        account_key: &str,
        message: &ProviderMessageId,
    ) -> Vec<OutboxEntry> {
        let key = (account_key.to_owned(), message.0.clone());
        let mut found: Vec<OutboxEntry> = self
            .by_message
            .get(&key)
            .into_iter()
            .flatten()
            .filter_map(|id| self.entries.get(id).cloned())
            .collect();
        found.sort_by_key(|e| e.seq);
        found
    }
}
