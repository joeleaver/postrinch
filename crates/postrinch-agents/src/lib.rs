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

pub mod client;
pub mod registry;
pub mod serve;
pub mod setup;
mod views;
pub mod wire;

pub use registry::{Agent, Agents, Reach, Setup};
pub use setup::App;
pub use wire::{Call, KEY_ENV};
