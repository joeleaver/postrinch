//! Listing a mailbox: the threads that carry a label, and how many of its
//! messages are unread.

use postrinch_model::{EmailAddress, ProviderThreadId, Timestamp};

use crate::keys::scoped;
use crate::messages::{THREAD, TYPE};
use crate::{Store, StoreResult, labels};

/// A thread in a mailbox listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadEntry {
    pub account: EmailAddress,
    pub thread: ProviderThreadId,
    /// When the thread's latest message arrived, whichever mailbox that
    /// message is in: a reply you sent moves the thread up the inbox, as in
    /// Gmail.
    pub latest: Timestamp,
}

impl Store {
    /// The threads in which some message carries the label, latest activity
    /// first. Empty for a label the account does not have.
    pub fn label_threads(
        &self,
        account: &EmailAddress,
        label_provider_id: &str,
    ) -> StoreResult<Vec<ThreadEntry>> {
        let Some(label) = self.find(labels::TYPE, "key", &scoped(account, label_provider_id))?
        else {
            return Ok(Vec::new());
        };
        let mut threads: Vec<u64> = Vec::new();
        for message in self.links(labels::TYPE, label.id(), "messages")? {
            if let Some(thread) = self.link(TYPE, message, "thread")? {
                threads.push(thread);
            }
        }
        threads.sort_unstable();
        threads.dedup();

        let prefix = scoped(account, "");
        let mut entries = Vec::with_capacity(threads.len());
        for thread in self.get_many(THREAD, &threads)? {
            let key = thread.str("key")?;
            let Some(provider_id) = key.strip_prefix(&prefix) else {
                continue;
            };
            let messages = self.links(THREAD, thread.id(), "messages")?;
            let mut latest = None;
            for message in self.get_many(TYPE, &messages)? {
                let received = message.datetime("received")?;
                latest = latest.max(Some(received));
            }
            if let Some(latest) = latest {
                entries.push(ThreadEntry {
                    account: account.clone(),
                    thread: ProviderThreadId(provider_id.to_owned()),
                    latest: Timestamp::from_millis(latest),
                });
            }
        }
        entries.sort_by(|a, b| {
            b.latest
                .cmp(&a.latest)
                .then_with(|| a.thread.cmp(&b.thread))
        });
        Ok(entries)
    }

    /// How many of the label's messages are unread. Zero for a label the
    /// account does not have.
    pub fn unread_count(
        &self,
        account: &EmailAddress,
        label_provider_id: &str,
    ) -> StoreResult<usize> {
        let Some(label) = self.find(labels::TYPE, "key", &scoped(account, label_provider_id))?
        else {
            return Ok(0);
        };
        let messages = self.links(labels::TYPE, label.id(), "messages")?;
        let mut unread = 0;
        for message in self.get_many(TYPE, &messages)? {
            if message.bool("unread")? {
                unread += 1;
            }
        }
        Ok(unread)
    }
}
