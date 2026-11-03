//! People: one object per address, shared by every account, so "everything
//! from this person" spans accounts.

use postrinch_model::Mailbox;

use crate::codec::FieldsBuilder;
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
                self.db.update(
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
}
