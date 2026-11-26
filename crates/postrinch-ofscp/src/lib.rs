//! OFSCP, the Open Federated Social Communications Protocol, for Postrinch.
//!
//! This is the protocol's core, with no I/O, no async runtime and no
//! Postrinch types (docs/ofscp.md, "Crates"):
//!
//! * [`ident`]: actors, provider authorities, local ids, object URIs and
//!   refs (spec §2.2).
//! * [`DmId`]: the id of a private conversation (§7.4).
//! * [`sign`]: the canonical strings, Ed25519 device keys, the signed
//!   request headers (§4.4) and the WebSocket `authenticate` signature (§7.1).
//! * [`wire`]: the REST payloads, as serde types (§3–§7, §10).
//! * [`ws`]: the WebSocket envelope, commands and events (§7.1).
//!
//! The spec is `lost-conn/ofscp`, v0.1. Every wire type round-trips the
//! spec's own example JSON, and the signing code reproduces the spec's
//! vectors byte for byte (see `tests/`). Calls (§9) and Web Push and
//! webhook delivery (§10) are not here yet: calls are a later wave, and a
//! desktop client receives notifications on its socket.
//!
//! The core is built so that it can move into one crate shared with
//! re-ofscp, the provider (docs/ofscp.md, "Shared with re-ofscp").

pub mod ident;
pub mod sign;
pub mod wire;
pub mod ws;

mod dm;

pub use dm::{DmId, DmIdError};
pub use ident::{Actor, Authority, IdentError, LocalId, ObjectRef, ObjectUri};

/// The OFSCP protocol version this crate speaks.
pub const PROTOCOL_VERSION: &str = "0.1.0";
