//! Labels despite joeleaver/rhypedb#27: after a reopen, a `@unique` lookup
//! misses a key that is the start of another (`…|Label_1` beside
//! `…|Label_10`), so an upsert made a second label with the same key.
//!
//! A label is therefore found by its key and, failing that, among its
//! account's labels, which are few; and copies made before this are
//! merged into one when the store opens. Only labels have keys like that:
//! Gmail's message and thread ids are all the same length.

use std::collections::{BTreeMap, HashMap};

use postrinch_model::EmailAddress;

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::scoped;
use crate::labels::TYPE;
use crate::messages::TYPE as MESSAGE;
use crate::{Store, StoreResult};

impl Store {
    /// An account's label by the provider's id.
    pub(crate) fn find_label(
        &self,
        account: &EmailAddress,
        provider_id: &str,
    ) -> StoreResult<Option<Fields>> {
        if let Some(found) = self.find(TYPE, "key", &scoped(account, provider_id))? {
            return Ok(Some(found));
        }
        let ids = self.links("Account", self.account_id(account)?, "labels")?;
        let mut labels = self.get_many(TYPE, &ids)?;
        labels.sort_by_key(Fields::id);
        for label in labels {
            if label.str("provider_id")? == provider_id {
                return Ok(Some(label));
            }
        }
        Ok(None)
    }

    /// Merge each account's copies of a label into its oldest: their
    /// messages and the person's changes move to it, then the copies go.
    /// Returns how many copies were removed.
    pub fn merge_duplicate_labels(&self) -> StoreResult<usize> {
        let mut removed = 0;
        for account in self.accounts()? {
            let ids = self.links("Account", self.account_id(&account.address)?, "labels")?;
            let mut by_id: BTreeMap<String, Vec<Fields>> = BTreeMap::new();
            for label in self.get_many(TYPE, &ids)? {
                by_id
                    .entry(label.str("provider_id")?)
                    .or_default()
                    .push(label);
            }
            for (_, mut copies) in by_id.into_iter().filter(|(_, c)| c.len() > 1) {
                copies.sort_by_key(Fields::id);
                let keep = copies.remove(0);
                let mut prefs = keep.opt_json::<serde_json::Value>("prefs")?;
                for copy in copies {
                    for message in self.links(TYPE, copy.id(), "messages")? {
                        self.db_link(MESSAGE, message, "labels", keep.id())?;
                    }
                    if prefs.is_none() {
                        prefs = copy.opt_json("prefs")?;
                    }
                    self.db_delete(TYPE, copy.id())?;
                    removed += 1;
                }
                if let Some(prefs) = prefs {
                    let update = FieldsBuilder::new().json("prefs", &prefs);
                    self.db_update(TYPE, keep.id(), update.build())?;
                }
            }
        }
        Ok(removed)
    }

    /// Objects of a type sharing a unique `field` value: how many objects
    /// in all, and how many are extra copies, by value.
    pub fn duplicate_keys(
        &self,
        type_name: &'static str,
        field: &str,
    ) -> StoreResult<(usize, HashMap<String, usize>)> {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut total = 0;
        for object in self.db.scan_type_projected(type_name, &[field])? {
            total += 1;
            *seen
                .entry(Fields::new(type_name, object).str(field)?)
                .or_default() += 1;
        }
        seen.retain(|_, n| *n > 1);
        seen.values_mut().for_each(|n| *n -= 1);
        Ok((total, seen))
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::{
        Account, Flags, Label, LabelKind, Message, MessageHeaders, Provider, ProviderMessageId,
        ProviderThreadId, Timestamp,
    };

    use super::*;

    #[test]
    fn copies_merge_into_the_oldest_with_their_messages() {
        let dir = tempfile::tempdir().unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            store
                .put_account(&Account::new(me.clone(), Provider::Google))
                .unwrap();
            for (id, name) in [("Label_1", "Receipts"), ("Label_10", "Travel")] {
                store
                    .put_label(&me, &Label::new(id, name, LabelKind::User))
                    .unwrap();
            }
        }
        let store = (0..50)
            .find_map(|_| {
                Store::open(dir.path())
                    .inspect_err(|_| std::thread::sleep(std::time::Duration::from_millis(100)))
                    .ok()
            })
            .expect("the store reopens");
        // What an upsert did before labels were found despite rhypedb#27:
        // a second label with the same key, carrying a message.
        let account = store.account_id(&me).unwrap();
        let copy = FieldsBuilder::new()
            .str("key", scoped(&me, "Label_1"))
            .relation("account", account)
            .str("provider_id", "Label_1")
            .str("name", "Receipts")
            .json("kind", &LabelKind::User);
        let Ok(copy) = store.db_create(TYPE, copy.build()) else {
            // The engine caught it: rhypedb#27 is fixed, nothing to merge.
            return;
        };
        let message = Message {
            account: me.clone(),
            id: ProviderMessageId("m1".into()),
            thread: ProviderThreadId("t1".into()),
            headers: MessageHeaders::default(),
            received: Timestamp::from_millis(100),
            labels: Vec::new(),
            flags: Flags::default(),
            snippet: String::new(),
            body_text: String::new(),
            downloaded: false,
            attachments: Vec::new(),
            size: 0,
        };
        store.put_message(&message, None).unwrap();
        let m1 = store
            .find(MESSAGE, "key", &scoped(&me, "m1"))
            .unwrap()
            .unwrap()
            .id();
        store.db_link(MESSAGE, m1, "labels", copy).unwrap();
        assert_eq!(store.duplicate_keys(TYPE, "key").unwrap().1.len(), 1);

        assert_eq!(store.merge_duplicate_labels().unwrap(), 1);
        assert!(store.duplicate_keys(TYPE, "key").unwrap().1.is_empty());
        assert_eq!(store.labels(&me).unwrap().len(), 2);
        let stored = &store
            .thread_messages(&me, &ProviderThreadId("t1".into()))
            .unwrap()[0];
        assert_eq!(
            stored.labels,
            ["Label_1"],
            "the message moved to the original"
        );
        assert_eq!(store.merge_duplicate_labels().unwrap(), 0);
    }
}
