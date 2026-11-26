//! Ed25519 device keys (spec §4.3) and signatures (§4.4.2, §7.1).

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use rand_core::CryptoRngCore;

/// A device's private signing key. It never leaves the device (§4.3.2); the
/// caller keeps its [`seed`](Self::seed) in the platform keychain.
///
/// Randomness is passed in, so this crate stays free of I/O: the transport
/// gives it the operating system's generator.
pub struct DeviceKey(SigningKey);

impl DeviceKey {
    /// A fresh key from a cryptographically secure generator.
    pub fn generate(rng: &mut impl CryptoRngCore) -> Self {
        Self(SigningKey::generate(rng))
    }

    /// The key whose 32-byte seed is `seed`.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }

    /// The 32-byte seed, for storing the key.
    pub fn seed(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// The public half, registered with the home provider.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }

    /// `base64(Ed25519_sign(key, utf8(canonical)))`, the value of
    /// `X-OFSCP-Signature` or of `authenticate.signature`.
    pub fn sign(&self, canonical: &str) -> String {
        B64.encode(self.0.sign(canonical.as_bytes()).to_bytes())
    }
}

impl fmt::Debug for DeviceKey {
    /// Shows the public key only, never the secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DeviceKey")
            .field(&self.public_key())
            .finish()
    }
}

/// An Ed25519 public key, as `public_key` carries it: the raw 32-byte key
/// in padded standard base64, 44 characters (§4.3.2).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    /// Decode a base64 public key. `None` if it is not padded standard
    /// base64 of exactly 32 bytes (a DER `SubjectPublicKeyInfo` is refused),
    /// or not a valid curve point.
    pub fn from_base64(b64: &str) -> Option<Self> {
        let bytes: [u8; 32] = B64.decode(b64).ok()?.try_into().ok()?;
        VerifyingKey::from_bytes(&bytes).ok().map(Self)
    }

    /// The base64 form sent on the wire.
    pub fn to_base64(&self) -> String {
        B64.encode(self.0.as_bytes())
    }

    /// Whether `signature_b64` is this key's signature over `canonical`.
    ///
    /// Verification is strict (it refuses small-order keys and a
    /// non-canonical `s`), which every honestly made Ed25519 signature
    /// passes.
    pub fn verify(&self, canonical: &str, signature_b64: &str) -> bool {
        let Ok(bytes) = B64.decode(signature_b64) else {
            return false;
        };
        let Ok(sig) = Signature::from_slice(&bytes) else {
            return false;
        };
        self.0.verify_strict(canonical.as_bytes(), &sig).is_ok()
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PublicKey").field(&self.to_base64()).finish()
    }
}

/// Bytes of entropy in a request nonce: 192 bits, above the 128 that §4.4.1
/// requires.
const NONCE_BYTES: usize = 24;

/// A fresh `X-OFSCP-Nonce` value: base64url, unpadded (§4.4.1).
pub fn generate_nonce(rng: &mut impl CryptoRngCore) -> String {
    let mut bytes = [0u8; NONCE_BYTES];
    rng.fill_bytes(&mut bytes);
    B64URL.encode(bytes)
}

/// The `X-OFSCP-Timestamp` (and `authenticate.timestamp`) for the moment
/// `now`: RFC 3339 in UTC with a `Z`, to the second (§4.4.1).
pub fn signing_timestamp(now: jiff::Timestamp) -> String {
    // A Timestamp is always UTC, so its fields need no time zone.
    now.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;

    #[test]
    fn generated_keys_sign_and_verify() {
        let key = DeviceKey::generate(&mut OsRng);
        let public = key.public_key();
        let sig = key.sign("hello");
        assert!(public.verify("hello", &sig));
        assert!(!public.verify("hellO", &sig));
        assert!(!public.verify("hello", "not base64!"));
        assert!(!public.verify("hello", &B64.encode([0u8; 10])));

        let again = DeviceKey::from_seed(&key.seed());
        assert_eq!(again.public_key(), public);
        assert_eq!(PublicKey::from_base64(&public.to_base64()), Some(public));
    }

    #[test]
    fn debug_hides_the_secret() {
        let key = DeviceKey::from_seed(&[7u8; 32]);
        let shown = format!("{key:?}");
        assert!(shown.contains(&key.public_key().to_base64()));
        assert!(!shown.contains(&B64.encode(key.seed())));
    }

    #[test]
    fn bad_public_keys() {
        assert_eq!(PublicKey::from_base64(""), None);
        assert_eq!(PublicKey::from_base64(&B64.encode([1u8; 31])), None);
        assert_eq!(PublicKey::from_base64("%%%"), None);
        // The spec's negative sample: DER SubjectPublicKeyInfo, not raw.
        let spki = "MCowBQYDK2VwAyEAGb9ECWmYfAJ6tS1j/Mdc6rjxPXK5+f6tcyLn4Bv7B1M=";
        assert_eq!(PublicKey::from_base64(spki), None);
        // The raw key inside it is fine; without its padding it is not.
        let raw = "Gb9ECWmYfAJ6tS1j/Mdc6rjxPXK5+f6tcyLn4Bv7B1M=";
        assert!(PublicKey::from_base64(raw).is_some());
        assert_eq!(PublicKey::from_base64(raw.trim_end_matches('=')), None);
    }

    #[test]
    fn nonces_are_long_unique_base64url() {
        let a = generate_nonce(&mut OsRng);
        let b = generate_nonce(&mut OsRng);
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
    }

    #[test]
    fn timestamps_are_utc_seconds() {
        let t: jiff::Timestamp = "2026-01-01T12:00:00.789+02:00".parse().unwrap();
        assert_eq!(signing_timestamp(t), "2026-01-01T10:00:00Z");
    }
}
