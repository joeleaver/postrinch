//! Finding messages: full-text words narrowed by who, where, when and what
//! state, newest first. The command layer turns a typed query (`from:`,
//! `label:`, `is:unread`, ...) into a [`Find`]; this file runs it.
//!
//! Each filter that has an index of its own narrows first (people through
//! their `sent`/`received` links, labels through theirs), the full-text
//! ranking then runs over only what is left, and the message's own small
//! fields (dates, flags, subject) are checked last.

use std::collections::{HashMap, HashSet};

use postrinch_model::{EmailAddress, ProviderMessageId, ProviderThreadId, Timestamp};

use crate::codec::Fields;
use crate::keys::scoped;
use crate::messages::{CONTENT, THREAD, TYPE};
use crate::{Store, StoreResult, ThreadEntry, labels};

/// How many full-text matches are looked at when nothing narrows them
/// first. Matches past this many are the least relevant ones.
const TEXT_LIMIT: usize = 5_000;
/// Up to how many candidates are read one by one rather than in a pass
/// over every message.
const BY_ONE: usize = 3_000;
/// The message fields a search checks.
const FIELDS: [&str; 7] = [
    "key",
    "received",
    "unread",
    "starred",
    "has_attachments",
    "subject",
    "size",
];

/// What to look for. Every field given must hold; an empty one is no
/// condition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Find {
    /// A full-text query in rhypedb's syntax (`+word`, `+"a phrase"`,
    /// `prefix*`), over subject, people and body.
    pub text: Option<String>,
    /// Full-text queries no message may match, each as `text` is written:
    /// Gmail's `-word`. A message is left out when it matches one.
    pub text_without: Vec<String>,
    /// Only these accounts' mail; empty for every account's.
    pub accounts: Vec<EmailAddress>,
    /// Each entry must match a sender's address or name (a part of it,
    /// any case).
    pub from: Vec<String>,
    /// Each entry must match a recipient's (To, Cc or Bcc).
    pub to: Vec<String>,
    /// No sender may match any of these, as `from`'s entries.
    pub not_from: Vec<String>,
    /// No recipient may match any of these, as `to`'s entries.
    pub not_to: Vec<String>,
    /// Each entry must appear in the subject (any case).
    pub subject: Vec<String>,
    /// Each entry is a label as it is in each account, by provider id; the
    /// message must carry one of the entry's labels, for every entry.
    pub labels: Vec<Vec<(EmailAddress, String)>>,
    /// Labels the message must not carry, as `labels`' entries.
    pub without: Vec<Vec<(EmailAddress, String)>>,
    pub unread: Option<bool>,
    pub starred: Option<bool>,
    pub attachments: Option<bool>,
    /// Received at or after.
    pub after: Option<Timestamp>,
    /// Received before.
    pub before: Option<Timestamp>,
    /// More than this many bytes.
    pub larger: Option<u64>,
    /// Fewer than this many bytes.
    pub smaller: Option<u64>,
    /// Only these messages, when given: for checking what a server's
    /// search found against the rest of a query.
    pub within: Option<Vec<(EmailAddress, ProviderMessageId)>>,
    /// Look in the trash and spam too (every other mailbox leaves them out).
    pub set_aside_too: bool,
    /// Every message the words match, not only the most relevant
    /// [`TEXT_LIMIT`]: for changing all of them, not for showing them.
    pub every: bool,
}

/// A message that matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub account: EmailAddress,
    pub id: ProviderMessageId,
    pub received: Timestamp,
    pub unread: bool,
    /// Full-text relevance (BM25, higher is better); 0 without words.
    pub score: f32,
    pub(crate) object: u64,
}

impl Store {
    /// Every message `find` matches, newest first.
    pub fn find_messages(&self, find: &Find) -> StoreResult<Vec<Found>> {
        let accounts = self.accounts_by_key()?;
        let wanted: HashSet<String> = find.accounts.iter().map(EmailAddress::key).collect();

        // What the indexed filters leave, if any of them was given.
        let mut narrowed: Option<HashSet<u64>> = None;
        let mut narrow = |set: HashSet<u64>| {
            narrowed = Some(match narrowed.take() {
                Some(had) => had.intersection(&set).copied().collect(),
                None => set,
            });
        };
        for part in &find.from {
            narrow(self.people_messages(part, "sent")?);
        }
        for part in &find.to {
            narrow(self.people_messages(part, "received")?);
        }
        for any in &find.labels {
            narrow(self.labelled(any)?);
        }
        if let Some(within) = &find.within {
            narrow(self.message_objects(within)?);
        }

        // Then the words. Ranking every match and keeping those the filters
        // left is cheap: a match leads to its message by a forward link.
        // Only when the words match more than TEXT_LIMIT messages is the
        // ranking restricted to the filtered ones up front, which costs a
        // reverse-link lookup per message.
        let mut scores: HashMap<u64, f32> = HashMap::new();
        let candidates: Option<HashSet<u64>> = match &find.text {
            Some(text) => {
                let limit = if find.every {
                    (self.db.count_type(CONTENT)? as usize).max(1)
                } else {
                    TEXT_LIMIT
                };
                let mut hits = self.text_hits(text, limit, None)?;
                if let Some(messages) = &narrowed
                    && hits.len() >= limit
                {
                    let contents = self.contents_of(messages)?;
                    hits = self.text_hits(text, contents.len().max(1), Some(&contents))?;
                }
                scores = hits;
                let matched = scores.keys().copied();
                Some(match &narrowed {
                    Some(messages) => matched.filter(|m| messages.contains(m)).collect(),
                    None => matched.collect(),
                })
            }
            None => narrowed,
        };

        let mut set_aside: HashSet<u64> = HashSet::new();
        if !find.set_aside_too {
            for account in accounts.values() {
                if wanted.is_empty() || wanted.contains(&account.key()) {
                    set_aside.extend(self.set_aside(account)?);
                }
            }
        }
        let mut without: HashSet<u64> = HashSet::new();
        for any in &find.without {
            without.extend(self.labelled(any)?);
        }
        for part in &find.not_from {
            without.extend(self.people_messages(part, "sent")?);
        }
        for part in &find.not_to {
            without.extend(self.people_messages(part, "received")?);
        }
        for text in &find.text_without {
            without.extend(self.text_matches(text, candidates.as_ref())?);
        }

        // A few candidates are read one by one; past BY_ONE, one pass over
        // every message's small fields is quicker.
        let keep = |id: u64| !set_aside.contains(&id) && !without.contains(&id);
        let objects: Vec<Fields> = match candidates {
            Some(ids) if ids.len() <= BY_ONE => {
                let ids: Vec<u64> = ids.into_iter().filter(|&id| keep(id)).collect();
                self.get_many(TYPE, &ids)?
            }
            candidates => self
                .db
                .scan_type_projected(TYPE, &FIELDS)?
                .into_iter()
                .map(|o| Fields::new(TYPE, o))
                .filter(|f| keep(f.id()))
                .filter(|f| candidates.as_ref().is_none_or(|c| c.contains(&f.id())))
                .collect(),
        };

        let subject: Vec<String> = find.subject.iter().map(|s| s.to_lowercase()).collect();
        let mut found = Vec::new();
        for fields in objects {
            let key = fields.str("key")?;
            let Some((account_key, id)) = key.split_once('\u{1f}') else {
                continue;
            };
            if !wanted.is_empty() && !wanted.contains(account_key) {
                continue;
            }
            let Some(account) = accounts.get(account_key) else {
                continue;
            };
            let received = fields.datetime("received")?;
            if find.after.is_some_and(|t| received < t.as_millis())
                || find.before.is_some_and(|t| received >= t.as_millis())
            {
                continue;
            }
            let unread = fields.bool("unread")?;
            let starred = fields.bool("starred")?;
            // Old stores may lack it; no attachments then.
            let attachments = fields.bool("has_attachments").unwrap_or(false);
            if find.unread.is_some_and(|v| unread != v)
                || find.starred.is_some_and(|v| starred != v)
                || find.attachments.is_some_and(|v| attachments != v)
            {
                continue;
            }
            // Old stores may lack it; no size is known then.
            let size = fields.opt_u64("size")?;
            if find.larger.is_some_and(|n| size.is_none_or(|s| s <= n))
                || find.smaller.is_some_and(|n| size.is_none_or(|s| s >= n))
            {
                continue;
            }
            if !subject.is_empty() {
                let have = fields
                    .opt_str("subject")?
                    .unwrap_or_default()
                    .to_lowercase();
                if !subject.iter().all(|s| have.contains(s.as_str())) {
                    continue;
                }
            }
            found.push(Found {
                account: account.clone(),
                id: ProviderMessageId(id.to_owned()),
                received: Timestamp::from_millis(received),
                unread,
                score: scores.get(&fields.id()).copied().unwrap_or(0.0),
                object: fields.id(),
            });
        }
        found.sort_by(|a, b| b.received.cmp(&a.received).then_with(|| a.id.cmp(&b.id)));
        Ok(found)
    }

    /// The conversation a found message is in.
    pub fn found_thread(&self, found: &Found) -> StoreResult<Option<ProviderThreadId>> {
        let Some(thread) = self.link(TYPE, found.object, "thread")? else {
            return Ok(None);
        };
        Ok(Some(ProviderThreadId(
            self.get(THREAD, thread)?.str("provider_id")?,
        )))
    }

    /// The conversations found messages are in, each once, as a listing
    /// has them (latest activity first), unread when a found message is.
    pub fn found_threads(&self, found: &[Found]) -> StoreResult<Vec<ThreadEntry>> {
        let mut by_account: HashMap<String, (EmailAddress, Vec<u64>)> = HashMap::new();
        let mut unread_threads = HashSet::new();
        for f in found {
            let Some(thread) = self.link(TYPE, f.object, "thread")? else {
                continue;
            };
            if f.unread {
                unread_threads.insert(thread);
            }
            by_account
                .entry(f.account.key())
                .or_insert_with(|| (f.account.clone(), Vec::new()))
                .1
                .push(thread);
        }
        let mut entries = Vec::new();
        for (account, mut threads) in by_account.into_values() {
            threads.sort_unstable();
            threads.dedup();
            entries.extend(self.thread_entries(&account, &threads, &unread_threads)?);
        }
        entries.sort_by(|a, b| {
            b.latest
                .cmp(&a.latest)
                .then_with(|| a.thread.cmp(&b.thread))
        });
        Ok(entries)
    }

    /// The accounts, by the key their objects' keys start with.
    pub(crate) fn accounts_by_key(&self) -> StoreResult<HashMap<String, EmailAddress>> {
        Ok(self
            .accounts()?
            .into_iter()
            .map(|a| (a.address.key(), a.address))
            .collect())
    }

    /// Messages linked through `relation` ("sent" or "received") to anyone
    /// whose address or name contains `part`.
    fn people_messages(&self, part: &str, relation: &str) -> StoreResult<HashSet<u64>> {
        let part = part.to_lowercase();
        let mut messages = HashSet::new();
        for object in self
            .db
            .scan_type_projected("Person", &["address", "name"])?
        {
            let person = Fields::new("Person", object);
            let address = person.str("address")?;
            let name = person.opt_str("name")?.unwrap_or_default().to_lowercase();
            if address.contains(&part) || name.contains(&part) {
                messages.extend(self.links("Person", person.id(), relation)?);
            }
        }
        Ok(messages)
    }

    /// Messages carrying any of `labels`.
    /// The objects of these messages, those that are stored.
    fn message_objects(
        &self,
        messages: &[(EmailAddress, ProviderMessageId)],
    ) -> StoreResult<HashSet<u64>> {
        let mut objects = HashSet::new();
        for (account, id) in messages {
            if let Some(found) = self.find(TYPE, "key", &scoped(account, &id.0))? {
                objects.insert(found.id());
            }
        }
        Ok(objects)
    }

    fn labelled(&self, labels: &[(EmailAddress, String)]) -> StoreResult<HashSet<u64>> {
        let mut messages = HashSet::new();
        for (account, provider_id) in labels {
            if let Some(label) = self.find_label(account, provider_id)? {
                messages.extend(self.links(labels::TYPE, label.id(), "messages")?);
            }
        }
        Ok(messages)
    }

    /// The best `limit` full-text matches (among `restrict`'s content
    /// objects, if given), as message ids with their scores.
    fn text_hits(
        &self,
        text: &str,
        limit: usize,
        restrict: Option<&HashSet<u64>>,
    ) -> StoreResult<HashMap<u64, f32>> {
        let result =
            self.db
                .fulltext_search(CONTENT, "search_text", text, limit, restrict, None)?;
        let mut hits = HashMap::with_capacity(result.hits.len());
        for hit in &result.hits {
            if let Some(message) = self.link(CONTENT, hit.object_id, "message")? {
                hits.insert(message, hit.score);
            }
        }
        Ok(hits)
    }

    /// Every message `text` matches (among `within`, when given).
    fn text_matches(&self, text: &str, within: Option<&HashSet<u64>>) -> StoreResult<HashSet<u64>> {
        let hits = match within {
            Some(messages) if messages.len() <= BY_ONE => {
                if messages.is_empty() {
                    return Ok(HashSet::new());
                }
                let contents = self.contents_of(messages)?;
                self.text_hits(text, contents.len().max(1), Some(&contents))?
            }
            _ => {
                let every = (self.db.count_type(CONTENT)? as usize).max(1);
                self.text_hits(text, every, None)?
            }
        };
        Ok(hits.into_keys().collect())
    }

    /// The content objects of `messages`, which the full-text index holds.
    fn contents_of(&self, messages: &HashSet<u64>) -> StoreResult<HashSet<u64>> {
        let mut contents = HashSet::with_capacity(messages.len());
        for &message in messages {
            contents.extend(self.links(TYPE, message, "content")?);
        }
        Ok(contents)
    }
}
