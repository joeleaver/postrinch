use std::collections::HashMap;

use postrinch_model::{EmailAddress, Label, LabelPrefs};

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::scoped;
use crate::{Store, StoreError, StoreResult};

pub(crate) const TYPE: &str = "Label";

pub(crate) fn decode(fields: &Fields) -> StoreResult<Label> {
    let mut label = Label::new(
        fields.str("provider_id")?,
        fields.str("name")?,
        fields.json("kind")?,
    );
    label.visibility = fields.opt_json("visibility")?.unwrap_or_default();
    label.color = fields.opt_json("color")?.flatten();
    Ok(label)
}

impl Store {
    /// Add a label to an account, or update its name, kind, visibility and
    /// colour. A label that already reads so is not written again: sync
    /// refreshes every label on every pass, and a label is linked one-to-one
    /// to its account, so a write is not free (rhypedb rewrites what links
    /// to what changed).
    pub fn put_label(&self, account: &EmailAddress, label: &Label) -> StoreResult<()> {
        let account_id = self.account_id(account)?;
        let key = scoped(account, &label.provider_id);
        let update = FieldsBuilder::new()
            .str("name", label.name.clone())
            .json("kind", &label.kind)
            .json("visibility", &label.visibility)
            .json("color", &label.color);
        let create = FieldsBuilder::new()
            .str("key", key.clone())
            .relation("account", account_id)
            .str("provider_id", label.provider_id.clone())
            .str("name", label.name.clone())
            .json("kind", &label.kind)
            .json("visibility", &label.visibility)
            .json("color", &label.color);
        // Found by key or among the account's labels (rhypedb#27, see
        // duplicates.rs), so a lookup that misses never makes a copy.
        match self.find_label(account, &label.provider_id)? {
            Some(existing) if decode(&existing)? == *label => {}
            Some(existing) => self.db_update(TYPE, existing.id(), update.build())?,
            None => {
                self.upsert(TYPE, "key", &key, create.build(), update.build())?;
            }
        }
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

    /// What the person changed about each of an account's labels on this
    /// computer, by provider id. Labels they never changed are left out.
    pub fn label_prefs(&self, account: &EmailAddress) -> StoreResult<HashMap<String, LabelPrefs>> {
        let account_id = self.account_id(account)?;
        let ids = self.links("Account", account_id, "labels")?;
        let mut prefs = HashMap::new();
        for fields in self.get_many(TYPE, &ids)? {
            if let Some(p) = fields.opt_json::<LabelPrefs>("prefs")? {
                prefs.insert(fields.str("provider_id")?, p);
            }
        }
        Ok(prefs)
    }

    /// Replace what the person changed about a label.
    pub fn set_label_prefs(
        &self,
        account: &EmailAddress,
        provider_id: &str,
        prefs: &LabelPrefs,
    ) -> StoreResult<()> {
        let label =
            self.find_label(account, provider_id)?
                .ok_or_else(|| StoreError::UnknownLabel {
                    account: account.clone(),
                    id: provider_id.to_owned(),
                })?;
        let update = FieldsBuilder::new().json("prefs", prefs);
        self.db_update(TYPE, label.id(), update.build())
    }

    /// Remove a label; messages that carried it lose it. Removing a label
    /// that is not there does nothing.
    pub fn remove_label(&self, account: &EmailAddress, provider_id: &str) -> StoreResult<()> {
        if let Some(label) = self.find_label(account, provider_id)? {
            self.db_delete(TYPE, label.id())?;
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
                self.find_label(account, id)?
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
    use postrinch_model::{Account, LabelColor, LabelKind, LabelVisibility, Provider, SystemLabel};

    use super::*;

    #[test]
    fn put_list_rename_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();

        let inbox = Label::new("INBOX", "Inbox", LabelKind::System(SystemLabel::Inbox));
        let mut work = Label::new("Label_1", "Work", LabelKind::User);
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

    #[test]
    fn keeps_visibility_colour_and_the_persons_prefs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.org").unwrap();
        store
            .put_account(&Account::new(me.clone(), Provider::Google))
            .unwrap();
        let mut label = Label::new("Label_1", "Receipts", LabelKind::User);
        label.visibility = LabelVisibility::ShowIfUnread;
        label.color = Some(LabelColor {
            background: "#16a766".into(),
            text: "#ffffff".into(),
        });
        store.put_label(&me, &label).unwrap();
        assert_eq!(store.labels(&me).unwrap(), vec![label.clone()]);
        assert!(store.label_prefs(&me).unwrap().is_empty());

        let prefs = LabelPrefs {
            name: Some("Bills".into()),
            shown: Some(false),
            pinned: true,
            color: Some("#2456b8".into()),
        };
        store.set_label_prefs(&me, "Label_1", &prefs).unwrap();
        // Sync refreshing the label leaves the person's changes alone.
        label.visibility = LabelVisibility::Show;
        store.put_label(&me, &label).unwrap();
        assert_eq!(store.labels(&me).unwrap(), vec![label.clone()]);
        assert_eq!(store.label_prefs(&me).unwrap()["Label_1"], prefs);

        // The same label again writes nothing.
        let before = store.writes();
        store.put_label(&me, &label).unwrap();
        assert_eq!(store.writes(), before);
        assert!(store.set_label_prefs(&me, "Label_9", &prefs).is_err());
    }
}
