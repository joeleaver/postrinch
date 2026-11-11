//! People: one object per address, shared by every account, so "everything
//! from this person" spans accounts.
//!
//! joeleaver/rhypedb#27: after a reopen, a `@unique` lookup misses an
//! address that is the start of another (`bob@ex.co` beside `bob@ex.com`),
//! so a second person was made for it. People are too many to look through
//! on every miss, so copies are merged into the oldest when the store opens
//! ([`Store::merge_duplicate_people`]), until the engine finds them itself.

use std::collections::HashMap;

use postrinch_model::Mailbox;

use crate::codec::{Fields, FieldsBuilder};
use crate::messages::TYPE as MESSAGE;
use crate::{Store, StoreResult};

const TYPE: &str = "Person";

impl Store {
    /// The store id of the person with `mailbox`'s address, created if new.
    /// A name is recorded the first time one is seen; later names do not
    /// overwrite it, so one odd `From:` cannot rename a contact.
    pub(crate) fn person_id(&self, mailbox: &Mailbox) -> StoreResult<u64> {
        let key = mailbox.address.key();
        if let Some(existing) = self.find(TYPE, "address", &key)? {
            if existing.opt_str("name")?.is_none()
                && let Some(name) = &mailbox.name
            {
                self.db_update(
                    TYPE,
                    existing.id(),
                    FieldsBuilder::new().str("name", name.clone()).build(),
                )?;
            }
            return Ok(existing.id());
        }
        let create = FieldsBuilder::new()
            .str("address", key.clone())
            .opt_str("name", mailbox.name.clone())
            .build();
        // A racing writer that created the person first is fine: upsert
        // turns the violation into an empty update.
        self.upsert(TYPE, "address", &key, create, FieldsBuilder::new().build())
    }

    /// Merge each address's copies of a person into its oldest: the
    /// messages they sent and received move to it, and its name is the
    /// first known. Returns how many copies were removed.
    pub fn merge_duplicate_people(&self) -> StoreResult<usize> {
        let mut by_address: HashMap<String, Vec<u64>> = HashMap::new();
        for object in self.db.scan_type_projected(TYPE, &["address"])? {
            let fields = Fields::new(TYPE, object);
            by_address
                .entry(fields.str("address")?)
                .or_default()
                .push(fields.id());
        }
        let mut removed = 0;
        for (_, mut copies) in by_address.into_iter().filter(|(_, c)| c.len() > 1) {
            copies.sort_unstable();
            let keep = copies.remove(0);
            let mut name = self.get(TYPE, keep)?.opt_str("name")?;
            for copy in copies {
                for message in self.links(TYPE, copy, "sent")? {
                    self.db_link(MESSAGE, message, "from", keep)?;
                }
                for message in self.links(TYPE, copy, "received")? {
                    self.db_link(MESSAGE, message, "recipients", keep)?;
                }
                if name.is_none() {
                    name = self.get(TYPE, copy)?.opt_str("name")?;
                    if let Some(name) = &name {
                        let update = FieldsBuilder::new().str("name", name.clone());
                        self.db_update(TYPE, keep, update.build())?;
                    }
                }
                self.db_delete(TYPE, copy)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub(crate) fn person_ids<'a>(
        &self,
        mailboxes: impl IntoIterator<Item = &'a Mailbox>,
    ) -> StoreResult<Vec<u64>> {
        let mut ids = Vec::new();
        for mailbox in mailboxes {
            let id = self.person_id(mailbox)?;
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::EmailAddress;

    use super::*;

    #[test]
    fn one_person_per_address_first_name_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let unnamed = Mailbox::new(None, EmailAddress::parse("Ada@example.org").unwrap());
        let named = Mailbox::new(
            Some("Ada".into()),
            EmailAddress::parse("ada@example.org").unwrap(),
        );
        let renamed = Mailbox::new(
            Some("Not Ada".into()),
            EmailAddress::parse("ada@example.org").unwrap(),
        );

        let a = store.person_id(&unnamed).unwrap();
        let b = store.person_id(&named).unwrap();
        let c = store.person_id(&renamed).unwrap();
        assert_eq!((a, a), (b, c));
        assert_eq!(
            store
                .get(TYPE, a)
                .unwrap()
                .opt_str("name")
                .unwrap()
                .as_deref(),
            Some("Ada")
        );
        assert_eq!(store.person_ids([&named, &renamed]).unwrap(), vec![a]);
    }

    #[test]
    fn copies_of_a_person_merge_into_the_oldest_with_their_mail() {
        use postrinch_model::{
            Flags, Message, MessageHeaders, ProviderMessageId, ProviderThreadId, Timestamp,
        };

        let dir = tempfile::tempdir().unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        let ada = Mailbox::new(None, EmailAddress::parse("ada@example.org").unwrap());
        // An address another starts with is what rhypedb#27 misses after a
        // reopen.
        let longer = Mailbox::new(None, EmailAddress::parse("ada@example.org.au").unwrap());
        let message = |id: &str, from: Vec<Mailbox>| Message {
            account: me.clone(),
            id: ProviderMessageId(id.into()),
            thread: ProviderThreadId("t1".into()),
            headers: MessageHeaders {
                from,
                ..Default::default()
            },
            received: Timestamp::from_millis(100),
            labels: Vec::new(),
            flags: Flags::default(),
            snippet: String::new(),
            body_text: String::new(),
            downloaded: false,
            attachments: Vec::new(),
            size: 0,
        };
        let original = {
            let store = Store::open(dir.path()).unwrap();
            store
                .put_account(&postrinch_model::Account::new(
                    me.clone(),
                    postrinch_model::Provider::Google,
                ))
                .unwrap();
            store.person_id(&longer).unwrap();
            store
                .put_message(&message("m1", vec![ada.clone()]), None)
                .unwrap();
            store.put_message(&message("m2", Vec::new()), None).unwrap();
            store.person_id(&ada).unwrap()
        };
        let store = (0..50)
            .find_map(|_| {
                Store::open(dir.path())
                    .inspect_err(|_| std::thread::sleep(std::time::Duration::from_millis(100)))
                    .ok()
            })
            .expect("the store reopens");
        // What a missed lookup made (rhypedb#27): a second Ada, named,
        // with a message of her own.
        let copy = FieldsBuilder::new()
            .str("address", ada.address.key())
            .str("name", "Ada")
            .build();
        let Ok(copy) = store.db_create(TYPE, copy) else {
            // The engine caught it: rhypedb#27 is fixed, nothing to merge.
            return;
        };
        let m2 = store
            .find(MESSAGE, "key", &crate::keys::scoped(&me, "m2"))
            .unwrap()
            .unwrap()
            .id();
        store.db_link(MESSAGE, m2, "from", copy).unwrap();

        assert_eq!(store.merge_duplicate_people().unwrap(), 1);
        assert_eq!(store.links(TYPE, original, "sent").unwrap().len(), 2);
        assert_eq!(
            store
                .get(TYPE, original)
                .unwrap()
                .opt_str("name")
                .unwrap()
                .as_deref(),
            Some("Ada"),
            "the copy's name, as the original had none"
        );
        assert_eq!(store.merge_duplicate_people().unwrap(), 0);
    }
}
