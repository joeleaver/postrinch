//! Who signs: an actor and one of its registered device keys (§4.3).

use ofscp_core::Actor;
use ofscp_core::signing::{DeviceKey, Signer};

/// An actor signed in on this device: its handle, the `key_id` its home
/// provider gave the device key, and the key itself. The same identity
/// signs requests to every provider (§4.4); remote providers check it
/// against the keys the home provider publishes (§4.6).
///
/// The caller keeps the key's [seed](DeviceKey::seed) somewhere safe, such
/// as the platform keychain, and rebuilds the identity from it.
#[derive(Debug)]
pub struct Identity {
    actor: Actor,
    key_id: String,
    key: DeviceKey,
}

impl Identity {
    /// The identity of `actor` signing with `key`, registered as `key_id`.
    pub fn new(actor: Actor, key_id: String, key: DeviceKey) -> Self {
        Self { actor, key_id, key }
    }

    /// Who this is.
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// The device key's id at the home provider.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// The device key.
    pub fn key(&self) -> &DeviceKey {
        &self.key
    }

    /// The signer for requests and the WebSocket sign-in.
    pub fn signer(&self) -> Signer<'_> {
        Signer {
            actor: &self.actor,
            key_id: &self.key_id,
            key: &self.key,
        }
    }
}
