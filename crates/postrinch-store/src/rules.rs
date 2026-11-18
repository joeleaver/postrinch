//! An account's mail rules, as this computer keeps them: the settings the
//! account's settings message holds, and what only this computer knows.

use postrinch_model::EmailAddress;
use postrinch_rules::{Local, Settings};

use crate::codec::FieldsBuilder;
use crate::{Store, StoreResult};

const TYPE: &str = "AccountRules";

impl Store {
    /// The account's rules, and what this computer knows of running them;
    /// both empty for an account that has none.
    pub fn rules(&self, account: &EmailAddress) -> StoreResult<(Settings, Local)> {
        match self.find(TYPE, "account", &account.key())? {
            Some(fields) => Ok((
                fields.opt_json("settings")?.unwrap_or_default(),
                fields.opt_json("local")?.unwrap_or_default(),
            )),
            None => Ok((Settings::default(), Local::default())),
        }
    }

    /// Change the account's rules and what is known of them, in one write
    /// that no other change to them comes between.
    pub fn update_rules<T>(
        &self,
        account: &EmailAddress,
        change: impl FnOnce(&mut Settings, &mut Local) -> T,
    ) -> StoreResult<T> {
        let _held = self.rules_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (mut settings, mut local) = self.rules(account)?;
        let answer = change(&mut settings, &mut local);
        let key = account.key();
        let fields = || {
            FieldsBuilder::new()
                .str("account", key.clone())
                .json("settings", &settings)
                .json("local", &local)
        };
        self.upsert(TYPE, "account", &key, fields().build(), fields().build())?;
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use postrinch_rules::Rule;

    use super::*;

    #[test]
    fn rules_are_kept_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.com").unwrap();
        let other = EmailAddress::parse("other@example.com").unwrap();
        assert_eq!(store.rules(&me).unwrap(), Default::default());
        store
            .update_rules(&me, |s, l| {
                let id = s.rules.take_id();
                s.rules.put(Rule::new(id, "School"));
                l.dirty = true;
            })
            .unwrap();
        let (settings, local) = store.rules(&me).unwrap();
        assert_eq!(settings.rules.rules[0].name, "School");
        assert!(local.dirty);
        assert!(store.rules(&other).unwrap().0.rules.rules.is_empty());
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.rules(&me).unwrap().0.rules.next_id, 2);
    }
}
