//! Private-conversation ids (spec §7.4, "Conversation id", byte-exact).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::Actor;

/// A two-party conversation's id: `dm_` and 64 lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DmId(String);

impl DmId {
    /// The id both participants derive, with no coordination (§7.4).
    ///
    /// Both actors are already canonical (the [`Actor`] type guarantees it),
    /// so step 1's lowercasing is a no-op: a canonical actor is all
    /// lowercase. The two are sorted by code point (byte order is the same
    /// for ASCII), LF-joined, hashed with SHA-256 and hex-encoded behind
    /// `dm_`. The order of the arguments does not matter.
    pub fn derive(a: &Actor, b: &Actor) -> Self {
        let (a, b) = (a.to_string(), b.to_string());
        let (first, second) = if a <= b { (a, b) } else { (b, a) };
        let digest = Sha256::digest(format!("{first}\n{second}").as_bytes());
        let mut out = String::with_capacity(3 + 64);
        out.push_str("dm_");
        for byte in digest {
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Self(out)
    }

    /// Accept an id received on the wire, if it has the §7.4 shape.
    pub fn parse(s: &str) -> Result<Self, DmIdError> {
        let hex = s.strip_prefix("dm_").ok_or(DmIdError)?;
        if hex.len() == 64 && hex.bytes().all(|b| HEX.contains(&b)) {
            Ok(Self(s.to_owned()))
        } else {
            Err(DmIdError)
        }
    }

    /// The id as sent.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// A received `dmId` does not match `^dm_[0-9a-f]{64}$`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("dmId must match ^dm_[0-9a-f]{{64}}$")]
pub struct DmIdError;

impl fmt::Display for DmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for DmId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DmId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published example in §7.4.
    const SPEC_DM_ID: &str = "dm_c2a3a0d4bc7aa54700d2f412c42fc0155df6071e502977e4988933eef7e46868";

    #[test]
    fn spec_vector() {
        let alice = Actor::parse_canonical("alice@a.com").unwrap();
        let bob = Actor::parse_canonical("bob@b.com").unwrap();
        assert_eq!(DmId::derive(&alice, &bob).as_str(), SPEC_DM_ID);
        assert_eq!(DmId::derive(&bob, &alice).as_str(), SPEC_DM_ID);
    }

    #[test]
    fn spellings_converge() {
        let a1 = Actor::canonicalize("alice@A.com.").unwrap();
        let a2 = Actor::parse_canonical("alice@a.com").unwrap();
        let bob = Actor::canonicalize("bob@B.COM:443").unwrap();
        assert_eq!(DmId::derive(&a1, &bob).as_str(), SPEC_DM_ID);
        assert_eq!(DmId::derive(&a1, &bob), DmId::derive(&a2, &bob));
    }

    #[test]
    fn parse_checks_shape() {
        assert_eq!(DmId::parse(SPEC_DM_ID).unwrap().as_str(), SPEC_DM_ID);
        let upper = SPEC_DM_ID.to_ascii_uppercase().replacen("DM_", "dm_", 1);
        for bad in ["", "dm_", "dm_abc", &SPEC_DM_ID[3..], &upper] {
            assert_eq!(DmId::parse(bad), Err(DmIdError), "{bad}");
        }
    }
}
