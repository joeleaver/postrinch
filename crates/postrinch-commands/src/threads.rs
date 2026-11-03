//! Listing a folder's threads and reading one.

use postrinch_model::{EmailAddress, Message, ProviderThreadId, ThreadSummary};
use serde::{Deserialize, Serialize};

use crate::{CommandResult, Commands, Folder};

/// A page of a folder's threads, latest activity first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadPage {
    /// How many threads the folder has in all.
    pub total: usize,
    pub offset: usize,
    pub threads: Vec<ThreadSummary>,
}

/// A thread with all its messages, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub summary: ThreadSummary,
    pub messages: Vec<Message>,
}

impl Commands {
    /// The folder's threads from `offset`, at most `limit` of them.
    pub fn list_threads(
        &self,
        folder: &Folder,
        offset: usize,
        limit: usize,
    ) -> CommandResult<ThreadPage> {
        let mut entries = Vec::new();
        for (account, label) in self.sources(folder)? {
            entries.extend(self.store.label_threads(&account, &label)?);
        }
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
            threads.extend(ThreadSummary::from_messages(&messages));
        }
        Ok(ThreadPage {
            total,
            offset,
            threads,
        })
    }

    /// A thread and its messages. `None` for a thread the store does not
    /// have.
    pub fn read_thread(
        &self,
        account: &EmailAddress,
        thread: &ProviderThreadId,
    ) -> CommandResult<Option<Thread>> {
        let messages = self.store.thread_messages(account, thread)?;
        Ok(ThreadSummary::from_messages(&messages).map(|summary| Thread { summary, messages }))
    }
}
