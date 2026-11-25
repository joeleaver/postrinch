use postrinch_model::{Account, AccountState, EmailAddress, ImapSettings, Provider};

use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreError, StoreResult};

const TYPE: &str = "Account";

fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Google => "google",
        Provider::Imap => "imap",
    }
}

fn decode(fields: &Fields) -> StoreResult<Account> {
    let address = fields.str("address")?;
    let address =
        EmailAddress::parse(&address).map_err(|e| StoreError::corrupt(TYPE, fields.id(), e))?;
    let provider = match fields.str("provider")?.as_str() {
        "google" => Provider::Google,
        "imap" => Provider::Imap,
        other => {
            return Err(StoreError::corrupt(
                TYPE,
                fields.id(),
                format!("unknown provider {other:?}"),
            ));
        }
    };
    Ok(Account {
        address,
        display_name: fields.opt_str("display_name")?,
        provider,
        state: fields.json("state")?,
        imap: fields.opt_json("imap")?,
    })
}

impl Store {
    /// Add an account, or replace what is stored about it. Its labels,
    /// messages and sync position are kept.
    pub fn put_account(&self, account: &Account) -> StoreResult<()> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let key = account.address.key();
        let fields = || {
            FieldsBuilder::new()
                .str("key", key.clone())
                .str("address", account.address.to_string())
                .opt_str("display_name", account.display_name.clone())
                .str("provider", provider_name(account.provider))
                .json("state", &account.state)
                .opt_json("imap", account.imap.as_ref())
        };
        self.upsert(TYPE, "key", &key, fields().build(), fields().build())?;
        Ok(())
    }

    pub fn account(&self, address: &EmailAddress) -> StoreResult<Option<Account>> {
        self.find(TYPE, "key", &address.key())?
            .as_ref()
            .map(decode)
            .transpose()
    }

    /// Every account, in the order they were added.
    pub fn accounts(&self) -> StoreResult<Vec<Account>> {
        let mut objects = self.db.scan_type(TYPE)?;
        objects.sort_by_key(|o| o.id);
        objects
            .into_iter()
            .map(|o| decode(&Fields::new(TYPE, o)))
            .collect()
    }

    pub fn set_account_state(
        &self,
        address: &EmailAddress,
        state: &AccountState,
    ) -> StoreResult<()> {
        let id = self.account_id(address)?;
        self.db_update(TYPE, id, FieldsBuilder::new().json("state", state).build())?;
        Ok(())
    }

    /// Replace an IMAP account's settings, and nothing else about it.
    /// Sending changes one of them by itself with
    /// [`Store::sent_copy_filed`] and [`Store::server_files_sent`].
    pub fn set_imap_settings(
        &self,
        address: &EmailAddress,
        settings: &ImapSettings,
    ) -> StoreResult<()> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let id = self.account_id(address)?;
        let fields = FieldsBuilder::new().json("imap", settings).build();
        self.db_update(TYPE, id, fields)?;
        Ok(())
    }

    /// Where the provider's incremental sync last got to, if it has run.
    pub fn sync_cursor(&self, address: &EmailAddress) -> StoreResult<Option<String>> {
        let id = self.account_id(address)?;
        self.get(TYPE, id)?.opt_str("sync_cursor")
    }

    /// Record the sync position. Call only once everything the provider
    /// reported before it is stored: a crash then repeats work, never skips
    /// it.
    pub fn set_sync_cursor(&self, address: &EmailAddress, cursor: Option<&str>) -> StoreResult<()> {
        let id = self.account_id(address)?;
        let fields = FieldsBuilder::new().opt_str("sync_cursor", cursor).build();
        self.db_update(TYPE, id, fields)?;
        Ok(())
    }

    /// Remove an account with its labels, threads, messages and IMAP
    /// folder state (the relations cascade). Raw messages stay in the blob
    /// store; [`Store::collect_garbage`] removes them.
    pub fn remove_account(&self, address: &EmailAddress) -> StoreResult<()> {
        let id = self.account_id(address)?;
        self.remove_calendars(address)?;
        self.remove_proposed_answers(address)?;
        self.db_delete(TYPE, id)?;
        Ok(())
    }

    /// Forget an account's mail but keep the account, its labels and its
    /// sign-in: every thread and message goes and the sync cursor (an IMAP
    /// account's position in each folder, and the copies it knew) is
    /// cleared, so the next sync rebuilds the local copy from the provider.
    /// Returns how many messages were removed. Raw messages stay in the
    /// blob store until [`Store::collect_garbage`].
    pub fn clear_mail(&self, address: &EmailAddress) -> StoreResult<usize> {
        let id = self.account_id(address)?;
        // Clear the cursor first: if this is interrupted, the next sync is a
        // full one, which removes or refetches whatever is left.
        self.set_sync_cursor(address, None)?;
        // An IMAP account's positions are per folder: they go the same way,
        // first, and so do the copies they knew.
        self.reset_imap_folders(address)?;
        let messages = self.message_ids(address)?.len();
        // A thread's messages are deleted with it (Message.thread cascades).
        for thread in self.links(TYPE, id, "threads")? {
            self.db_delete("Thread", thread)?;
        }
        Ok(messages)
    }

    pub(crate) fn account_id(&self, address: &EmailAddress) -> StoreResult<u64> {
        self.find(TYPE, "key", &address.key())?
            .map(|f| f.id())
            .ok_or_else(|| StoreError::UnknownAccount(address.clone()))
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::SignInReason;

    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn put_get_and_update() {
        let (_dir, store) = store();
        let address = EmailAddress::parse("Ada@Example.org").unwrap();
        let mut account = Account::new(address.clone(), Provider::Google);
        store.put_account(&account).unwrap();

        account.display_name = Some("Ada".into());
        account.needs_sign_in(SignInReason::NoCredential);
        store.put_account(&account).unwrap();

        let lookup = EmailAddress::parse("ada@example.org").unwrap();
        assert_eq!(store.account(&lookup).unwrap(), Some(account.clone()));
        assert_eq!(store.accounts().unwrap(), vec![account]);
    }

    #[test]
    fn sync_cursor_round_trip() {
        let (_dir, store) = store();
        let address = EmailAddress::parse("ada@example.org").unwrap();
        store
            .put_account(&Account::new(address.clone(), Provider::Google))
            .unwrap();
        assert_eq!(store.sync_cursor(&address).unwrap(), None);
        store.set_sync_cursor(&address, Some("12345")).unwrap();
        assert_eq!(
            store.sync_cursor(&address).unwrap().as_deref(),
            Some("12345")
        );
    }

    #[test]
    fn imap_settings_change_alone() {
        use postrinch_model::{FolderRoles, Security, Server, SignInMethod};

        let (_dir, store) = store();
        let address = EmailAddress::parse("ada@example.org").unwrap();
        let server = |host: &str, port| Server {
            host: host.into(),
            port,
            security: Security::Tls,
            username: "ada".into(),
        };
        let mut settings = ImapSettings {
            imap: server("imap.example.org", 993),
            smtp: server("smtp.example.org", 465),
            sign_in: SignInMethod::Password,
            roles: FolderRoles::default(),
            server_files_sent: true,
        };
        let mut account = Account::new(address.clone(), Provider::Imap);
        account.display_name = Some("Ada".into());
        account.imap = Some(settings.clone());
        store.put_account(&account).unwrap();
        store.set_sync_cursor(&address, Some("7")).unwrap();

        settings.server_files_sent = false;
        store.set_imap_settings(&address, &settings).unwrap();
        account.imap = Some(settings);
        assert_eq!(store.account(&address).unwrap(), Some(account));
        assert_eq!(store.sync_cursor(&address).unwrap().as_deref(), Some("7"));
    }

    #[test]
    fn unknown_account_is_an_error() {
        let (_dir, store) = store();
        let address = EmailAddress::parse("nobody@example.org").unwrap();
        assert!(matches!(
            store.sync_cursor(&address),
            Err(StoreError::UnknownAccount(_))
        ));
        assert_eq!(store.account(&address).unwrap(), None);
    }
}
