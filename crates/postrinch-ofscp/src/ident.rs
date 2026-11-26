//! The three identifier forms of OFSCP (spec §2.2): actors, local ids and
//! object URIs, plus the provider authority that actors and URIs are built on.
//!
//! An actor is the pair *(handle, domain)*. It is never reduced to a bare
//! handle: `alice@a.example` and `alice@b.example` are two people
//! (lost-conn/ofscp#10).
//!
//! Two parsers are offered, matching the two choices §2.2 gives a receiver:
//!
//! * `parse_canonical` **rejects** any spelling that is not already
//!   canonical, for fields the spec says MUST be canonical.
//! * `canonicalize` **converts** a recognisable spelling (upper-case domain,
//!   trailing dot, explicit `:443`) to canonical form, for received fields
//!   where conversion is allowed. Deserializing uses this one.
//!
//! A handle is never lowercased: §2.2.1 makes it byte-identical everywhere,
//! so `Alice@a.example` is refused rather than converted. Internationalized
//! domain names are accepted only in their ASCII (`xn--`) form; a Unicode
//! spelling is refused, which §2.2 permits.
//!
//! This module mirrors `re-ofscp-types::actor` so the two can become one
//! shared crate (docs/ofscp.md, "Shared with re-ofscp").

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why an identifier was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentError {
    /// The handle does not match `^[a-z0-9_-]{3,32}$` (§2.2.1).
    #[error("handle must match ^[a-z0-9_-]{{3,32}}$")]
    Handle,
    /// The value has no `@` separating handle and domain.
    #[error("actor must have the form handle@domain")]
    MissingAt,
    /// The authority is malformed (bad label, bad port, non-ASCII, ...).
    #[error("malformed provider authority")]
    Authority,
    /// The value is well-formed but not in canonical form.
    #[error("identifier is not in canonical form")]
    NotCanonical,
    /// A local id does not match `^[A-Za-z0-9_-]{1,128}$` (§2.2).
    #[error("local id must match ^[A-Za-z0-9_-]{{1,128}}$")]
    LocalId,
    /// An object URI is not an absolute `https://` URL on a valid authority.
    #[error("object URI must be an absolute https:// URL")]
    ObjectUri,
}

/// Whether `handle` matches `^[a-z0-9_-]{3,32}$` (§2.2.1).
pub fn is_valid_handle(handle: &str) -> bool {
    (3..=32).contains(&handle.len())
        && handle
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A provider authority in canonical form: lowercase host, no trailing dot,
/// `:port` only when the port is not 443 (§2.2, §4.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Authority(String);

impl Authority {
    /// Accept `s` only if it is already canonical.
    pub fn parse_canonical(s: &str) -> Result<Self, IdentError> {
        let canonical = Self::canonicalize(s)?;
        if canonical.0 == s {
            Ok(canonical)
        } else {
            Err(IdentError::NotCanonical)
        }
    }

    /// Convert a recognisable spelling of an authority to canonical form.
    pub fn canonicalize(s: &str) -> Result<Self, IdentError> {
        if s.is_empty() || !s.is_ascii() {
            return Err(IdentError::Authority);
        }
        let lower = s.to_ascii_lowercase();
        let (host, port) = match lower.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (lower.as_str(), None),
        };
        let host = host.strip_suffix('.').unwrap_or(host);
        if !is_valid_hostname(host) {
            return Err(IdentError::Authority);
        }
        let Some(port) = port else {
            return Ok(Self(host.to_owned()));
        };
        // Digits only, no sign, no leading zero, within u16.
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.starts_with('0') {
            return Err(IdentError::Authority);
        }
        let n: u16 = port.parse().map_err(|_| IdentError::Authority)?;
        if n == 443 {
            Ok(Self(host.to_owned()))
        } else {
            Ok(Self(format!("{host}:{n}")))
        }
    }

    /// The canonical spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Authority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// A user anywhere in the network, `handle@domain` (§2.2), always canonical.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Actor {
    handle: String,
    domain: Authority,
}

impl Actor {
    /// An actor from a handle and its home provider.
    pub fn new(handle: &str, domain: Authority) -> Result<Self, IdentError> {
        if !is_valid_handle(handle) {
            return Err(IdentError::Handle);
        }
        Ok(Self {
            handle: handle.to_owned(),
            domain,
        })
    }

    /// Accept `s` only if it is already canonical.
    pub fn parse_canonical(s: &str) -> Result<Self, IdentError> {
        let (handle, domain) = s.split_once('@').ok_or(IdentError::MissingAt)?;
        Self::new(handle, Authority::parse_canonical(domain)?)
    }

    /// Convert a recognisable spelling to canonical form (the domain only;
    /// the handle must already be valid).
    pub fn canonicalize(s: &str) -> Result<Self, IdentError> {
        let (handle, domain) = s.split_once('@').ok_or(IdentError::MissingAt)?;
        Self::new(handle, Authority::canonicalize(domain)?)
    }

    /// The handle, unique on its home provider.
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// The home provider.
    pub fn domain(&self) -> &Authority {
        &self.domain
    }

    /// Whether this actor's home provider is `authority`.
    pub fn is_local_to(&self, authority: &Authority) -> bool {
        &self.domain == authority
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.handle, self.domain)
    }
}

/// An object's id on the provider that hosts it (§2.2): opaque, path-safe,
/// meaningful only together with that provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(String);

impl LocalId {
    /// Accept `s` if it matches `^[A-Za-z0-9_-]{1,128}$`.
    pub fn new(s: &str) -> Result<Self, IdentError> {
        if is_local_id(s) {
            Ok(Self(s.to_owned()))
        } else {
            Err(IdentError::LocalId)
        }
    }

    /// The id as sent.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_local_id(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl fmt::Display for LocalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An absolute HTTPS URL naming an object across providers (§2.2), such as
/// `https://a.example/api/groups/grp_1`. Its authority is canonicalized; the
/// path is kept exactly as received.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectUri {
    authority: Authority,
    path: String,
}

impl ObjectUri {
    /// Parse `https://{authority}{path}`, converting the authority to its
    /// canonical form.
    pub fn parse(s: &str) -> Result<Self, IdentError> {
        let rest = s.strip_prefix("https://").ok_or(IdentError::ObjectUri)?;
        let (authority, path) = match rest.find(['/', '?', '#']) {
            Some(i) => rest.split_at(i),
            None => (rest, ""),
        };
        let authority = Authority::canonicalize(authority).map_err(|_| IdentError::ObjectUri)?;
        Ok(Self {
            authority,
            path: path.to_owned(),
        })
    }

    /// The provider that hosts the object.
    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    /// Everything after the authority, as received (may be empty).
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl fmt::Display for ObjectUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "https://{}{}", self.authority, self.path)
    }
}

/// A field documented as a *ref* (§2.2): a local id on the provider that
/// receives it, or an object URI. The two never overlap, since a local id
/// cannot contain `:` or `/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ObjectRef {
    /// An object on the provider receiving the request.
    Local(LocalId),
    /// An object named across providers.
    Uri(ObjectUri),
}

impl ObjectRef {
    /// Parse a ref.
    pub fn parse(s: &str) -> Result<Self, IdentError> {
        if is_local_id(s) {
            Ok(Self::Local(LocalId(s.to_owned())))
        } else {
            ObjectUri::parse(s).map(Self::Uri)
        }
    }
}

impl fmt::Display for ObjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(id) => id.fmt(f),
            Self::Uri(uri) => uri.fmt(f),
        }
    }
}

/// Serialize as the display string; deserialize through the named parser.
macro_rules! string_serde {
    ($ty:ty, $parse:path) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                $parse(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

string_serde!(Authority, Authority::canonicalize);
string_serde!(Actor, Actor::canonicalize);
string_serde!(LocalId, LocalId::new);
string_serde!(ObjectUri, ObjectUri::parse);
string_serde!(ObjectRef, ObjectRef::parse);

#[cfg(test)]
#[path = "ident_tests.rs"]
mod tests;
