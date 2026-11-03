//! Listing a mailbox: the threads that carry a label, and how each label
//! is used.

use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, LabelKind, ProviderThreadId, SystemLabel, Timestamp};

use crate::keys::scoped;
use crate::messages::{THREAD, TYPE};
use crate::{Store, StoreResult, labels};

/// How a label is used, counted over its messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LabelStats {
    pub messages: usize,
    pub unread: usize,
    /// Messages also in the inbox.
    pub in_inbox: usize,
    /// Messages out of the inbox with no other label the person made: filed
    /// here and nowhere else, as in a folder.
    pub filed_only_here: usize,
}

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

    /// How each of an account's labels is used, by provider id.
    pub fn label_stats(&self, account: &EmailAddress) -> StoreResult<HashMap<String, LabelStats>> {
        let account_id = self.account_id(account)?;
        let mut members: Vec<(String, LabelKind, Vec<u64>)> = Vec::new();
        for label in self.get_many(labels::TYPE, &self.links("Account", account_id, "labels")?)? {
            let messages = self.links(labels::TYPE, label.id(), "messages")?;
            members.push((label.str("provider_id")?, label.json("kind")?, messages));
        }
        let inbox: HashSet<u64> = members
            .iter()
            .filter(|(_, kind, _)| *kind == LabelKind::System(SystemLabel::Inbox))
            .flat_map(|(_, _, ids)| ids.iter().copied())
            .collect();
        // How many of the person's own labels each message carries.
        let mut own_labels: HashMap<u64, usize> = HashMap::new();
        for (_, kind, ids) in &members {
            if *kind == LabelKind::User {
                for id in ids {
                    *own_labels.entry(*id).or_default() += 1;
                }
            }
        }
        let mut all: Vec<u64> = members
            .iter()
            .flat_map(|(_, _, ids)| ids.iter().copied())
            .collect();
        all.sort_unstable();
        all.dedup();
        let mut unread = HashSet::new();
        for message in self.get_many(TYPE, &all)? {
            if message.bool("unread")? {
                unread.insert(message.id());
            }
        }

        let mut stats = HashMap::new();
        for (provider_id, _, ids) in members {
            let mut s = LabelStats {
                messages: ids.len(),
                ..Default::default()
            };
            for id in &ids {
                s.unread += usize::from(unread.contains(id));
                if inbox.contains(id) {
                    s.in_inbox += 1;
                } else if own_labels.get(id).copied().unwrap_or(0) <= 1 {
                    s.filed_only_here += 1;
                }
            }
            stats.insert(provider_id, s);
        }
        Ok(stats)
    }
}
