//! Agents: the apps the person lets read and change their mail, through
//! `postrinch-mcp`.
//!
//! Each agent has a name, a key (shown once, kept here only as its SHA-256)
//! and the accounts it may see ([`registry`]). An app launches
//! `postrinch-mcp` with the key in `POSTRINCH_AGENT_KEY`; the bridge
//! connects to the running app over loopback ([`client`]), and the app checks
//! the key and answers each call through the command layer limited to the
//! agent's accounts ([`serve`]). The agents file is read again on every
//! call, so removing an agent or narrowing it takes effect at once.

mod change;
pub mod client;
mod gmail;
mod later;
pub mod registry;
pub mod rule_shape;
pub mod serve;
pub mod setup;
mod views;
mod views_rules;
pub mod wire;

pub use gmail::Gmail;
pub use registry::{Agent, Agents, Reach, Setup};
pub use setup::App;
pub use wire::{Call, Changed, KEY_ENV, ProposedFile};
