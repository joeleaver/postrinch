//! Listing a mailbox: the threads that carry a label, and how each label
//! is used.

use std::collections::{HashMap, HashSet};

use postrinch_model::{
    EmailAddress, LabelKind, ProviderMessageId, ProviderThreadId, SystemLabel, Timestamp,
};

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

/// A message in a mailbox listing, as one message at a time lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageEntry {
    pub account: EmailAddress,
    pub thread: ProviderThreadId,
    pub message: ProviderMessageId,
    pub received: Timestamp,
}

impl Store {
    /// The messages that carry the label, newest first; spam and trash are
    /// left out unless the label is one of them, as in
    /// [`Store::label_threads`]. Empty for a label the account does not
    /// have.
    pub fn label_messages(
        &self,
        account: &EmailAddress,
        label_provider_id: &str,
    ) -> StoreResult<Vec<MessageEntry>> {
        let Some(label) = self.find_label(account, label_provider_id)? else {
            return Ok(Vec::new());
        };
        let kind: LabelKind = label.json("kind")?;
        let set_aside = if is_set_aside(&kind) {
            HashSet::new()
        } else {
            self.set_aside(account)?
        };
        let ids: Vec<u64> = self
            .links(labels::TYPE, label.id(), "messages")?
            .into_iter()
            .filter(|m| !set_aside.contains(m))
            .collect();
        let prefix = scoped(account, "");
        let mut threads: HashMap<u64, Option<ProviderThreadId>> = HashMap::new();
        let mut entries = Vec::with_capacity(ids.len());
        for fields in self.get_many_projected(TYPE, &ids, &["key", "received"])? {
            let Some(message) = fields.str("key")?.strip_prefix(&prefix).map(str::to_owned) else {
                continue;
            };
            let Some(thread_id) = self.link(TYPE, fields.id(), "thread")? else {
                continue;
            };
            let thread = match threads.get(&thread_id) {
                Some(found) => found.clone(),
                None => {
                    let key = self.get(THREAD, thread_id)?.str("key")?;
                    let found = key
                        .strip_prefix(&prefix)
                        .map(|t| ProviderThreadId(t.to_owned()));
                    threads.insert(thread_id, found.clone());
                    found
                }
            };
            let Some(thread) = thread else {
                continue;
            };
            entries.push(MessageEntry {
                account: account.clone(),
                thread,
                message: ProviderMessageId(message),
                received: Timestamp::from_millis(fields.datetime("received")?),
            });
        }
        entries.sort_by(|a, b| {
            b.received
                .cmp(&a.received)
                .then_with(|| a.message.cmp(&b.message))
        });
        Ok(entries)
    }

    /// The threads in which some message carries the label, latest activity
    /// first. Empty for a label the account does not have.
    pub fn label_threads(
        &self,
        account: &EmailAddress,
        label_provider_id: &str,
    ) -> StoreResult<Vec<ThreadEntry>> {
        let Some(label) = self.find_label(account, label_provider_id)? else {
            return Ok(Vec::new());
        };
        let kind: LabelKind = label.json("kind")?;
        let set_aside = if is_set_aside(&kind) {
            HashSet::new()
        } else {
            self.set_aside(account)?
        };
        let mut threads: Vec<u64> = Vec::new();
        for message in self.links(labels::TYPE, label.id(), "messages")? {
            if set_aside.contains(&message) {
                continue;
            }
            if let Some(thread) = self.link(TYPE, message, "thread")? {
                threads.push(thread);
            }
        }
        threads.sort_unstable();
        threads.dedup();

        let prefix = scoped(account, "");
        let mut entries = Vec::with_capacity(threads.len());
        for thread in self.get_many_projected(THREAD, &threads, &["key", "latest"])? {
            let key = thread.str("key")?;
            let Some(provider_id) = key.strip_prefix(&prefix) else {
                continue;
            };
            if let Some(latest) = self.thread_latest(&thread)? {
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

    /// How each of an account's labels is used, by provider id. Counting
    /// reads every labelled message, so the answer is kept until the next
    /// write: the sidebar and the thread list ask for it one after the
    /// other.
    pub fn label_stats(&self, account: &EmailAddress) -> StoreResult<HashMap<String, LabelStats>> {
        let as_of = self.writes();
        let key = account.key();
        if let Some((when, stats)) = self
            .stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            && *when == as_of
        {
            return Ok(stats.clone());
        }
        let stats = self.count_label_stats(account)?;
        self.stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (as_of, stats.clone()));
        Ok(stats)
    }

    fn count_label_stats(
        &self,
        account: &EmailAddress,
    ) -> StoreResult<HashMap<String, LabelStats>> {
        let account_id = self.account_id(account)?;
        let set_aside = self.set_aside(account)?;
        let mut members: Vec<(String, LabelKind, Vec<u64>)> = Vec::new();
        for label in self.get_many(labels::TYPE, &self.links("Account", account_id, "labels")?)? {
            let kind: LabelKind = label.json("kind")?;
            let mut messages = self.links(labels::TYPE, label.id(), "messages")?;
            if !is_set_aside(&kind) {
                messages.retain(|m| !set_aside.contains(m));
            }
            members.push((label.str("provider_id")?, kind, messages));
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
        for message in self.get_many_projected(TYPE, &all, &["unread"])? {
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

    /// The account's messages in the trash or spam, which every other
    /// mailbox leaves out, as Gmail's do.
    pub(crate) fn set_aside(&self, account: &EmailAddress) -> StoreResult<HashSet<u64>> {
        let mut found = HashSet::new();
        for label in self.get_many(
            labels::TYPE,
            &self.links("Account", self.account_id(account)?, "labels")?,
        )? {
            if is_set_aside(&label.json("kind")?) {
                found.extend(self.links(labels::TYPE, label.id(), "messages")?);
            }
        }
        Ok(found)
    }
}

/// The trash and spam: mailboxes whose mail no other mailbox shows.
fn is_set_aside(kind: &LabelKind) -> bool {
    matches!(
        kind,
        LabelKind::System(SystemLabel::Trash | SystemLabel::Spam)
    )
}
