//! Where device keys are kept: the OS keychain, service `postrinch.ofscp`,
//! keyed by the actor lower-cased (docs/ofscp.md, "Accounts and sign-in").
//! The password is never kept; the key's id is also in the store
//! ([`SocialAccount::key_id`](postrinch_store::SocialAccount::key_id)).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use postrinch_ofscp::Identity;
use postrinch_ofscp::ofscp_core::Actor;
use postrinch_ofscp::ofscp_core::signing::DeviceKey;
use postrinch_store::social_account_key;
use postrinch_sync_core::{Keychain, KeychainResult, MemorySecrets};
use serde::{Deserialize, Serialize};

use crate::{SocialError, SocialResult};

/// A place to keep each social account's device key. Calls may block (the
/// keychain talks to a system service), so the engine runs them on a
/// blocking thread.
pub trait DeviceKeys: Send + Sync + 'static {
    /// The entry kept under `key` (an actor lower-cased), as written.
    fn get(&self, key: &str) -> KeychainResult<Option<String>>;
    /// Keep `entry` under `key`, replacing what was there.
    fn set(&self, key: &str, entry: &str) -> KeychainResult<()>;
    /// Remove the entry; removing one that is not there is not an error.
    fn delete(&self, key: &str) -> KeychainResult<()>;
}

/// The OS keychain. Use [`Keychain::OFSCP`].
impl DeviceKeys for Keychain {
    fn get(&self, key: &str) -> KeychainResult<Option<String>> {
        self.get_key(key)
    }

    fn set(&self, key: &str, entry: &str) -> KeychainResult<()> {
        self.set_key(key, entry)
    }

    fn delete(&self, key: &str) -> KeychainResult<()> {
        self.delete_key(key)
    }
}

/// Keys in memory, for tests.
impl DeviceKeys for MemorySecrets {
    fn get(&self, key: &str) -> KeychainResult<Option<String>> {
        self.get_key(key)
    }

    fn set(&self, key: &str, entry: &str) -> KeychainResult<()> {
        self.set_key(key, entry)
    }

    fn delete(&self, key: &str) -> KeychainResult<()> {
        self.delete_key(key)
    }
}

/// What one keychain entry holds: the key's id with its seed, so a key
/// kept for one registration is never used under another's id (a sign-in
/// interrupted between the keychain and the store).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    key_id: String,
    /// The Ed25519 seed, standard base64.
    seed: String,
}

/// The keychain entry for `identity`.
pub(crate) fn entry_for(identity: &Identity) -> String {
    let entry = Entry {
        key_id: identity.key_id().to_owned(),
        seed: STANDARD.encode(*identity.key().seed()),
    };
    serde_json::to_string(&entry).expect("a device key entry always serializes")
}

/// The identity a keychain entry holds for `actor`, if it is the key
/// registered as `key_id`; `None` when the entry is for another key.
pub(crate) fn identity_from(
    actor: &Actor,
    key_id: &str,
    entry: &str,
) -> SocialResult<Option<Identity>> {
    let damaged = |reason: String| SocialError::DamagedKey {
        actor: actor.to_string(),
        reason,
    };
    let entry: Entry = serde_json::from_str(entry).map_err(|e| damaged(e.to_string()))?;
    if entry.key_id != key_id {
        return Ok(None);
    }
    let seed = STANDARD
        .decode(entry.seed.as_bytes())
        .map_err(|e| damaged(e.to_string()))?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| damaged("the seed is not 32 bytes".to_owned()))?;
    Ok(Some(Identity::new(
        actor.clone(),
        entry.key_id,
        DeviceKey::from_seed(&seed),
    )))
}

/// The keychain key of `actor`'s entry.
pub(crate) fn key_of(actor: &Actor) -> String {
    social_account_key(&actor.to_string())
}

#[cfg(test)]
mod tests {
    use postrinch_ofscp::ofscp_core::Authority;

    use super::*;

    fn identity(key_id: &str) -> Identity {
        let actor = Actor::new("ada", Authority::canonicalize("a.example").unwrap()).unwrap();
        Identity::new(actor, key_id.to_owned(), DeviceKey::from_seed(&[7; 32]))
    }

    #[test]
    fn an_entry_gives_back_its_identity() {
        let kept = identity("key_1");
        let entry = entry_for(&kept);
        let back = identity_from(kept.actor(), "key_1", &entry)
            .unwrap()
            .expect("the same key");
        assert_eq!(back.key_id(), "key_1");
        assert_eq!(*back.key().seed(), [7; 32]);
    }

    #[test]
    fn an_entry_for_another_key_is_not_used() {
        let kept = identity("key_1");
        let entry = entry_for(&kept);
        assert!(
            identity_from(kept.actor(), "key_2", &entry)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_damaged_entry_says_so() {
        let kept = identity("key_1");
        let short = r#"{"keyId":"key_1","seed":"AAAA"}"#;
        let error = identity_from(kept.actor(), "key_1", short).unwrap_err();
        assert!(error.to_string().contains("not 32 bytes"), "{error}");
        assert!(identity_from(kept.actor(), "key_1", "nonsense").is_err());
    }
}
