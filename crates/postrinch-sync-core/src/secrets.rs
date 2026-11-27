//! Where secrets are kept: the OS keychain, one entry per account and
//! service. Google accounts keep their refresh token under
//! `postrinch.google`; IMAP accounts keep their password or refresh token
//! under `postrinch.imap` (`postrinch_sync::imap::ImapSecret`).

use std::collections::HashMap;
use std::sync::Mutex;

use postrinch_model::EmailAddress;

use crate::{KeychainError, KeychainResult};

/// A place to keep one secret per account. Calls may block (the keychain
/// talks to a system service), so async callers run them on a blocking
/// thread.
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, account: &EmailAddress) -> KeychainResult<Option<String>>;
    fn set(&self, account: &EmailAddress, secret: &str) -> KeychainResult<()>;
    /// Remove the secret. Removing one that is not there is not an error.
    fn delete(&self, account: &EmailAddress) -> KeychainResult<()>;
}

/// The OS keychain: Keychain Services on macOS, Credential Manager on
/// Windows, the Secret Service elsewhere. Entries are under a service name
/// ([`Keychain::GOOGLE`], [`Keychain::IMAP`]), keyed by the account's
/// address.
///
/// On Windows the credential is kept on this machine only. Credential
/// Manager's default would roam it to every machine the person signs in to
/// on a domain, and Postrinch's secrets stay where its mail is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keychain {
    service: &'static str,
}

impl Keychain {
    /// Google accounts' refresh tokens.
    pub const GOOGLE: Keychain = Keychain {
        service: "postrinch.google",
    };

    /// IMAP accounts' passwords and refresh tokens, as JSON.
    pub const IMAP: Keychain = Keychain {
        service: "postrinch.imap",
    };

    /// OFSCP accounts' device keys, keyed by the actor lower-cased
    /// (docs/ofscp.md, "Accounts and sign-in").
    pub const OFSCP: Keychain = Keychain {
        service: "postrinch.ofscp",
    };

    /// The service name the entries are kept under.
    pub fn service(&self) -> &'static str {
        self.service
    }

    /// The secret kept under `key`, an account's key in this service (an
    /// address's [`EmailAddress::key`], an OFSCP actor lower-cased).
    pub fn get_key(&self, key: &str) -> KeychainResult<Option<String>> {
        match self.entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(KeychainError::new(e.to_string())),
        }
    }

    /// Keep `secret` under `key`, replacing what was there.
    pub fn set_key(&self, key: &str, secret: &str) -> KeychainResult<()> {
        #[cfg(all(unix, not(target_os = "macos")))]
        crate::secret_service::ensure_default_collection()?;
        self.entry(key)?
            .set_password(secret)
            .map_err(|e| KeychainError::new(e.to_string()))
    }

    /// Remove the secret under `key`; one that is not there is not an
    /// error.
    pub fn delete_key(&self, key: &str) -> KeychainResult<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KeychainError::new(e.to_string())),
        }
    }

    fn entry(&self, key: &str) -> KeychainResult<keyring_core::Entry> {
        // `keyring` chooses and installs the platform's store on first use.
        if let Err(e) = keyring::Entry::store_status() {
            return Err(KeychainError::new(e.to_string()));
        }
        let modifiers: HashMap<&str, &str> = if cfg!(windows) {
            HashMap::from([("persistence", "Local")])
        } else {
            HashMap::new()
        };
        keyring_core::Entry::new_with_modifiers(self.service, key, &modifiers)
            .map_err(|e| KeychainError::new(e.to_string()))
    }
}

impl SecretStore for Keychain {
    fn get(&self, account: &EmailAddress) -> KeychainResult<Option<String>> {
        self.get_key(&account.key())
    }

    fn set(&self, account: &EmailAddress, secret: &str) -> KeychainResult<()> {
        self.set_key(&account.key(), secret)
    }

    fn delete(&self, account: &EmailAddress) -> KeychainResult<()> {
        self.delete_key(&account.key())
    }
}

/// Secrets in memory, gone when the process ends. For tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl MemorySecrets {
    /// The secret kept under `key`, as [`Keychain::get_key`].
    pub fn get_key(&self, key: &str) -> KeychainResult<Option<String>> {
        Ok(self.lock().get(key).cloned())
    }

    /// Keep `secret` under `key`, as [`Keychain::set_key`].
    pub fn set_key(&self, key: &str, secret: &str) -> KeychainResult<()> {
        self.lock().insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    /// Remove the secret under `key`, as [`Keychain::delete_key`].
    pub fn delete_key(&self, key: &str) -> KeychainResult<()> {
        self.lock().remove(key);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, account: &EmailAddress) -> KeychainResult<Option<String>> {
        self.get_key(&account.key())
    }

    fn set(&self, account: &EmailAddress, secret: &str) -> KeychainResult<()> {
        self.set_key(&account.key(), secret)
    }

    fn delete(&self, account: &EmailAddress) -> KeychainResult<()> {
        self.delete_key(&account.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_secrets_by_key_and_by_address_are_one_entry() {
        let secrets = MemorySecrets::default();
        let address = EmailAddress::parse("Ada@Example.com").unwrap();
        secrets.set(&address, "token").unwrap();
        assert_eq!(
            secrets.get_key(&address.key()).unwrap().as_deref(),
            Some("token")
        );
        secrets.set_key("alice@a.example", "seed").unwrap();
        assert_eq!(
            secrets.get_key("alice@a.example").unwrap().as_deref(),
            Some("seed")
        );
        secrets.delete_key("alice@a.example").unwrap();
        secrets.delete_key("alice@a.example").unwrap();
        assert_eq!(secrets.get_key("alice@a.example").unwrap(), None);
        assert_eq!(secrets.get(&address).unwrap().as_deref(), Some("token"));
    }
}
