//! Where secrets are kept: the OS keychain, one entry per account and
//! service. Google accounts keep their refresh token under
//! `postrinch.google`; IMAP accounts keep their password or refresh token
//! under `postrinch.imap` ([`crate::imap::ImapSecret`]).

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

    /// The service name the entries are kept under.
    pub fn service(&self) -> &'static str {
        self.service
    }

    fn entry(&self, account: &EmailAddress) -> SyncResult<keyring_core::Entry> {
        // `keyring` chooses and installs the platform's store on first use.
        if let Err(e) = keyring::Entry::store_status() {
            return Err(SyncError::Keychain(e.to_string()));
        }
        let modifiers: HashMap<&str, &str> = if cfg!(windows) {
            HashMap::from([("persistence", "Local")])
        } else {
            HashMap::new()
        };
        keyring_core::Entry::new_with_modifiers(self.service, &account.key(), &modifiers)
            .map_err(|e| SyncError::Keychain(e.to_string()))
    }
}

impl SecretStore for Keychain {
    fn get(&self, account: &EmailAddress) -> SyncResult<Option<String>> {
        match self.entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SyncError::Keychain(e.to_string())),
        }
    }

    fn set(&self, account: &EmailAddress, secret: &str) -> SyncResult<()> {
        #[cfg(all(unix, not(target_os = "macos")))]
        crate::secret_service::ensure_default_collection()?;
        self.entry(account)?
            .set_password(secret)
            .map_err(|e| SyncError::Keychain(e.to_string()))
    }

    fn delete(&self, account: &EmailAddress) -> SyncResult<()> {
        match self.entry(account)?.delete_credential() {
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
