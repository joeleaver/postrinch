//! The WebSocket protocol (spec §7.1): the envelope every frame uses, the
//! commands a client sends, the events a provider sends, and the close
//! codes.
//!
//! Frames decode in two steps: the envelope, then its `data` by `type`. A
//! `type` this version does not know decodes as `Other` and keeps its data,
//! since clients must ignore unknown types (§2.3). Call events (§9) are among
//! those for now: calls are a later wave (docs/ofscp.md, decision 2).

mod command;
mod event;

pub use command::*;
pub use event::*;

use serde::{Deserialize, Serialize};

use crate::wire::Timestamp;

/// Close code: authentication failed, the actor or provider is refused by
/// federation policy, or the key could not be confirmed. The key was not
/// revoked; the client may reconnect later (§7.1).
pub const CLOSE_AUTH_FAILED: u16 = 4001;

/// Close code: the connection's key was revoked, or its account no longer
/// exists. The client must not reconnect with the same key (§7.1).
pub const CLOSE_REVOKED: u16 = 4003;

/// One frame: the envelope (§7.1) around a command or an event.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope<T> {
    /// Unique within the connection.
    pub id: String,
    /// When it was sent. Events always carry it. Spec main requires it on
    /// commands too, so a client sends it (lost-conn/ofscp#27 makes it
    /// optional there).
    pub ts: Option<Timestamp>,
    /// The `id` of the command this answers.
    pub correlation_id: Option<String>,
    /// The command or event.
    pub body: T,
}

/// A command or event: a `type` and its `data`.
pub trait Body: Sized {
    /// Decode `data` by `kind`. An unknown kind is not an error.
    fn decode(kind: &str, data: serde_json::Value) -> Result<Self, serde_json::Error>;

    /// The `type` and `data` to send.
    fn encode(&self) -> Result<(String, serde_json::Value), serde_json::Error>;
}

/// Why a frame could not be read.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// It is not a JSON envelope.
    #[error("not an OFSCP frame: {0}")]
    Envelope(#[source] serde_json::Error),
    /// Its `data` does not fit its `type`.
    #[error("bad data for {kind}: {source}")]
    Data {
        /// The frame's `type`.
        kind: String,
        /// What did not fit.
        #[source]
        source: serde_json::Error,
    },
}

/// The envelope as JSON, before `data` is decoded.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFrame {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ts: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    data: serde_json::Value,
}

impl<T: Body> Envelope<T> {
    /// Read a frame's text.
    pub fn decode(text: &str) -> Result<Self, DecodeError> {
        let raw: RawFrame = serde_json::from_str(text).map_err(DecodeError::Envelope)?;
        let body = T::decode(&raw.kind, raw.data).map_err(|source| DecodeError::Data {
            kind: raw.kind,
            source,
        })?;
        Ok(Self {
            id: raw.id,
            ts: raw.ts,
            correlation_id: raw.correlation_id,
            body,
        })
    }

    /// The frame's text, to send.
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        let (kind, data) = self.body.encode()?;
        serde_json::to_string(&RawFrame {
            id: self.id.clone(),
            kind,
            ts: self.ts,
            correlation_id: self.correlation_id.clone(),
            data,
        })
    }
}

/// Define a command or event enum: each variant a `type` and its data, plus
/// `Other` for types this version does not know.
macro_rules! ws_body {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident($data:ty) = $kind:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub enum $name {
            $( $(#[$vmeta])* $variant($data), )+
            /// A type this version does not know, kept as received.
            Other {
                /// Its `type`.
                kind: String,
                /// Its `data`.
                data: serde_json::Value,
            },
        }

        impl $name {
            /// Every standard `type` this enum knows.
            pub const KINDS: &'static [&'static str] = &[$($kind),+];

            /// The `type` on the wire.
            pub fn kind(&self) -> &str {
                match self {
                    $( Self::$variant(_) => $kind, )+
                    Self::Other { kind, .. } => kind,
                }
            }
        }

        impl $crate::ws::Body for $name {
            fn decode(kind: &str, data: serde_json::Value) -> Result<Self, serde_json::Error> {
                Ok(match kind {
                    $( $kind => Self::$variant(serde_json::from_value($crate::ws::known_data(data))?), )+
                    other => Self::Other { kind: other.to_owned(), data },
                })
            }

            fn encode(&self) -> Result<(String, serde_json::Value), serde_json::Error> {
                Ok(match self {
                    $( Self::$variant(data) => ($kind.to_owned(), serde_json::to_value(data)?), )+
                    Self::Other { kind, data } => (kind.clone(), data.clone()),
                })
            }
        }
    };
}
pub(crate) use ws_body;

/// The `data` of a standard type, which is always an object: one that is
/// absent (as `ping` and `pong` may send it) is empty.
pub(crate) fn known_data(data: serde_json::Value) -> serde_json::Value {
    match data {
        serde_json::Value::Null => serde_json::Value::Object(serde_json::Map::new()),
        data => data,
    }
}

/// The `data` of `ping` and `pong`: an empty object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

#[cfg(test)]
#[path = "ws_tests.rs"]
mod tests;
