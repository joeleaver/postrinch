//! The REST payloads of OFSCP v0.1 as serde types, following the spec's
//! JSON Schemas (`schemas/v0.1`).
//!
//! Conventions, from the spec:
//!
//! * **Unknown fields are ignored** (§2.3), so no type denies them.
//! * **Enumerations stay open** where a newer provider may add values (a
//!   MINOR version, §2.1): each has an `Other` variant that keeps the value,
//!   so one new value never makes a whole response unreadable. Enumerations
//!   in requests the client sends, whose values the spec fixes, are closed.
//! * **Optional fields** are `Option`s that are left out when `None`, so
//!   what is sent is what the schema allows.
//! * **`metadata` and `attachments`**, required lists, read as empty when a
//!   provider leaves them out: an absent list and an empty one mean the same,
//!   and refusing a whole object over it would lose the rest.
//! * Field names are camelCase, except the auth and key payloads of §4,
//!   which are snake_case (§2.5).

mod auth;
mod common;
mod discovery;
mod dm;
mod group;
mod membership;
mod message;
mod notify;
mod problem;
mod profile;

pub use auth::*;
pub use common::*;
pub use discovery::*;
pub use dm::*;
pub use group::*;
pub use membership::*;
pub use message::*;
pub use notify::*;
pub use problem::*;
pub use profile::*;

/// An RFC 3339 instant (§2.5).
pub use jiff::Timestamp;

/// Define an open enumeration: known values as variants, anything else kept
/// in `Other`. It (de)serializes as its string.
macro_rules! open_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $value:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
            /// A value this version does not know (§2.3). Build values with
            /// `From<&str>`, which never puts a known value here.
            Other(String),
        }

        impl $name {
            /// The value as sent on the wire.
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $value, )+
                    Self::Other(s) => s,
                }
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                match s {
                    $( $value => Self::$variant, )+
                    other => Self::Other(other.to_owned()),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
                Ok(Self::from(s.as_ref()))
            }
        }
    };
}
pub(crate) use open_enum;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_enums_keep_unknown_values() {
        let tier: Tier = serde_json::from_str("\"members-plus\"").unwrap();
        assert_eq!(tier, Tier::Other("members-plus".into()));
        assert_eq!(serde_json::to_string(&tier).unwrap(), "\"members-plus\"");
        assert_eq!(Tier::from("private"), Tier::Private);
        assert_eq!(Tier::Discoverable.to_string(), "discoverable");
    }
}
