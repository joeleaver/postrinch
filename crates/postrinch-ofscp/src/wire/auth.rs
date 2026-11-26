//! Sign-in, device keys, the keys endpoint and guest accounts (spec §4).
//!
//! These payloads use snake_case, the deliberate exception of §2.5, except
//! the login, registration and guest-claim requests, whose fields are single
//! words or camelCase, and the guest-upgrade answer.
//!
//! A `public_key` is kept as the string received; decode it with
//! [`crate::sign::PublicKey::from_base64`]. It is the raw 32-byte key in
//! padded standard base64, never a DER `SubjectPublicKeyInfo` (§4.3.2,
//! lost-conn/ofscp#28); a key that is not counts as unusable.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Actor;

use super::{KeyAlgorithm, Timestamp, UserProfile};

/// `POST /api/auth/login` (§4.1.2).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    /// The account's handle, without the domain.
    pub handle: String,
    /// The account's password.
    pub password: String,
}

/// `POST /api/auth/register` (§4.1.1).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationRequest {
    /// The handle to register (§2.2.1).
    pub handle: String,
    /// The new account's password (§4.1.4).
    pub password: String,
    /// An address for password recovery (§4.1.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_email: Option<String>,
    /// An account invite, for a provider whose registration is by invite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_token: Option<String>,
}

/// `POST /api/me/claim`: turn a guest into a full account (§4.8.1).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestClaimRequest {
    /// The new handle (not starting with `guest_`).
    pub handle: String,
    /// The new password.
    pub password: String,
    /// The new display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// An account invite, for a provider whose registration is by invite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_token: Option<String>,
}

/// Debug output that never shows a password.
macro_rules! redacted_debug {
    ($ty:ty, $($field:ident),+) => {
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($ty))
                    $( .field(stringify!($field), &self.$field) )+
                    .field("password", &"<redacted>")
                    .finish_non_exhaustive()
            }
        }
    };
}

redacted_debug!(LoginRequest, handle);
redacted_debug!(RegistrationRequest, handle, recovery_email);
redacted_debug!(GuestClaimRequest, handle, display_name);

/// The answer to login or registration: a one-use token for registering a
/// device key (§4.2).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapResponse {
    /// The token, sent as `Authorization: Bearer`.
    pub bootstrap_token: String,
    /// Always `bootstrap`.
    pub token_type: String,
    /// Seconds until the token expires (300 in v0.1).
    pub expires_in: u64,
}

impl fmt::Debug for BootstrapResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BootstrapResponse")
            .field("bootstrap_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// `POST /api/auth/device-keys` (§4.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKeyRegistration {
    /// The device's public key, base64.
    pub public_key: String,
    /// Always Ed25519 in v0.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<KeyAlgorithm>,
    /// A name the person recognises, such as "Postrinch on Linux".
    pub device_name: String,
}

/// The answer to a device-key registration (§4.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKeyResponse {
    /// The key's id, sent as `X-OFSCP-Key-ID` from now on.
    pub key_id: String,
    /// When the key was registered.
    pub created_at: Timestamp,
}

/// One of your own device keys, as `GET /api/auth/device-keys` lists it
/// (§4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKeyInfo {
    /// The key's id.
    pub key_id: String,
    /// The key's algorithm.
    pub algorithm: KeyAlgorithm,
    /// The public key, base64.
    pub public_key: String,
    /// The name given when it was registered.
    pub device_name: String,
    /// When it was registered.
    pub created_at: Timestamp,
}

/// `GET /api/auth/device-keys` (§4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKeysResponse {
    /// Your registered, unrevoked keys.
    pub keys: Vec<DeviceKeyInfo>,
}

/// A user's published key, as the keys endpoint gives it (§4.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedKey {
    /// The key's id.
    pub key_id: String,
    /// The key's algorithm.
    pub algorithm: KeyAlgorithm,
    /// The public key, base64.
    pub public_key: String,
    /// When it was registered.
    pub created_at: Timestamp,
}

/// `GET /.well-known/ofscp/users/{handle}/keys` (§4.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserKeysResponse {
    /// Whose keys these are.
    pub actor: Actor,
    /// The keys currently valid.
    pub keys: Vec<PublishedKey>,
    /// How long a verifier may cache the answer.
    pub cache_until: Timestamp,
}

/// `POST /api/invites/{token}/guest`: become a guest of a group (§5.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestCreateRequest {
    /// The guest's display name.
    #[serde(
        default,
        rename = "displayName",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Option<String>,
    /// The guest device's public key, base64.
    pub public_key: String,
    /// Always Ed25519 in v0.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<KeyAlgorithm>,
    /// A name the person recognises.
    pub device_name: String,
}

/// The answer to a guest claim or merge (§4.8.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestUpgradeResponse {
    /// The full account the guest became.
    pub actor: Actor,
    /// The device key's id, unchanged.
    pub key_id: String,
    /// The new account's profile.
    pub profile: UserProfile,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_stay_out_of_debug() {
        let login = LoginRequest {
            handle: "alice".into(),
            password: "hunter22".into(),
        };
        let shown = format!("{login:?}");
        assert!(shown.contains("alice") && !shown.contains("hunter22"));

        let token = BootstrapResponse {
            bootstrap_token: "tok_secret".into(),
            token_type: "bootstrap".into(),
            expires_in: 300,
        };
        assert!(!format!("{token:?}").contains("tok_secret"));
    }
}
