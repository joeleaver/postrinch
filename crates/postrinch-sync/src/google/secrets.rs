//! Where refresh tokens are kept: the OS keychain, one entry per account.

use std::collections::HashMap;
use std::sync::Mutex;

use postrinch_model::EmailAddress;

use crate::{SyncError, SyncResult};

/// A place to keep one secret per account. Calls may block (the keychain
/// talks to a system service), so async callers run them on a blocking
/// thread.
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, account: &EmailAddress) -> SyncResult<Option<String>>;
    fn set(&self, account: &EmailAddress, secret: &str) -> SyncResult<()>;
    /// Remove the secret. Removing one that is not there is not an error.
    fn delete(&self, account: &EmailAddress) -> SyncResult<()>;
}

/// The OS keychain: Keychain Services on macOS, Credential Manager on
/// Windows, the Secret Service elsewhere. Entries are under the service
/// name `postrinch.google`, keyed by the account's address.
///
/// On Windows the credential is kept on this machine only. Credential
/// Manager's default would roam it to every machine the person signs in to
/// on a domain, and Postrinch's tokens stay where its mail is.
pub struct Keychain;

const SERVICE: &str = "postrinch.google";

impl Keychain {
    fn entry(account: &EmailAddress) -> SyncResult<keyring_core::Entry> {
        // `keyring` chooses and installs the platform's store on first use.
        if let Err(e) = keyring::Entry::store_status() {
            return Err(SyncError::Keychain(e.to_string()));
        }
        let modifiers: HashMap<&str, &str> = if cfg!(windows) {
            HashMap::from([("persistence", "Local")])
        } else {
            HashMap::new()
        };
        keyring_core::Entry::new_with_modifiers(SERVICE, &account.key(), &modifiers)
            .map_err(|e| SyncError::Keychain(e.to_string()))
    }
}

impl SecretStore for Keychain {
    fn get(&self, account: &EmailAddress) -> SyncResult<Option<String>> {
        match Self::entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SyncError::Keychain(e.to_string())),
        }
    }

    fn set(&self, account: &EmailAddress, secret: &str) -> SyncResult<()> {
        Self::entry(account)?
            .set_password(secret)
            .map_err(|e| SyncError::Keychain(e.to_string()))
    }

    fn delete(&self, account: &EmailAddress) -> SyncResult<()> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SyncError::Keychain(e.to_string())),
        }
    }
}

/// Secrets in memory, gone when the process ends. For tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, account: &EmailAddress) -> SyncResult<Option<String>> {
        Ok(self.0.lock().unwrap().get(&account.key()).cloned())
    }

    fn set(&self, account: &EmailAddress, secret: &str) -> SyncResult<()> {
        self.0
            .lock()
            .unwrap()
            .insert(account.key(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, account: &EmailAddress) -> SyncResult<()> {
        self.0.lock().unwrap().remove(&account.key());
        Ok(())
    }
}
