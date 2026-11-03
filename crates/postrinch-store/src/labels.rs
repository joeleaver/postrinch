use postrinch_model::{EmailAddress, Label};

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::scoped;
use crate::{Store, StoreError, StoreResult};

pub(crate) const TYPE: &str = "Label";

pub(crate) fn decode(fields: &Fields) -> StoreResult<Label> {
    Ok(Label {
        provider_id: fields.str("provider_id")?,
        name: fields.str("name")?,
        kind: fields.json("kind")?,
    })
}

impl Store {
    /// Add a label to an account, or update its name and kind.
    pub fn put_label(&self, account: &EmailAddress, label: &Label) -> StoreResult<()> {
        let account_id = self.account_id(account)?;
        let key = scoped(account, &label.provider_id);
        let update = FieldsBuilder::new()
            .str("name", label.name.clone())
            .json("kind", &label.kind);
        let create = FieldsBuilder::new()
            .str("key", key.clone())
            .relation("account", account_id)
            .str("provider_id", label.provider_id.clone())
            .str("name", label.name.clone())
            .json("kind", &label.kind);
        self.upsert(TYPE, "key", &key, create.build(), update.build())?;
        Ok(())
    }

    /// An account's labels, sorted by name.
    pub fn labels(&self, account: &EmailAddress) -> StoreResult<Vec<Label>> {
        let account_id = self.account_id(account)?;
        let ids = self.links("Account", account_id, "labels")?;
        let mut labels = self
            .get_many(TYPE, &ids)?
            .iter()
            .map(decode)
            .collect::<StoreResult<Vec<_>>>()?;
        labels.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(labels)
    }

    /// Remove a label; messages that carried it lose it. Removing a label
    /// that is not there does nothing.
    pub fn remove_label(&self, account: &EmailAddress, provider_id: &str) -> StoreResult<()> {
        if let Some(label) = self.find(TYPE, "key", &scoped(account, provider_id))? {
            self.db.delete(TYPE, label.id())?;
        }
        Ok(())
    }

    /// The store ids of an account's labels, by provider id, in order.
    pub(crate) fn label_ids(
        &self,
        account: &EmailAddress,
        provider_ids: &[String],
    ) -> StoreResult<Vec<u64>> {
        provider_ids
            .iter()
            .map(|id| {
                self.find(TYPE, "key", &scoped(account, id))?
                    .map(|f| f.id())
                    .ok_or_else(|| StoreError::UnknownLabel {
                        account: account.clone(),
                        id: id.clone(),
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::{Account, LabelKind, Provider, SystemLabel};

    use super::*;

    #[test]
    fn put_list_rename_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();

        let inbox = Label {
            provider_id: "INBOX".into(),
            name: "Inbox".into(),
            kind: LabelKind::System(SystemLabel::Inbox),
        };
        let mut work = Label {
            provider_id: "Label_1".into(),
            name: "Work".into(),
            kind: LabelKind::User,
        };
        store.put_label(&me, &inbox).unwrap();
        store.put_label(&me, &work).unwrap();
        work.name = "Clients".into();
        store.put_label(&me, &work).unwrap();

        assert_eq!(
            store.labels(&me).unwrap(),
            vec![work.clone(), inbox.clone()]
        );

        store.remove_label(&me, "Label_1").unwrap();
        store.remove_label(&me, "Label_1").unwrap();
        assert_eq!(store.labels(&me).unwrap(), vec![inbox]);
        assert!(matches!(
            store.label_ids(&me, &["Label_1".into()]),
            Err(StoreError::UnknownLabel { .. })
        ));
    }
}
