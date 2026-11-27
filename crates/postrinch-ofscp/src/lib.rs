//! Postrinch's OFSCP transport: how it talks to OFSCP providers, on top of
//! [`ofscp_core`], which holds the protocol itself (identifiers, signing,
//! wire types, WebSocket frames). This crate keeps no store and knows no
//! Postrinch types; the sync engine (`postrinch-social`) builds on it.
//!
//! - [`Transport`]: what an application keeps one of: the connection pool,
//!   the trust settings, the limits, and each provider's [clock](ProviderClock).
//! - [`Endpoint`]: one provider, by its canonical authority. Everything goes
//!   over `https://{authority}` and `wss://{authority}/api/ws` (§3.2, §7.1).
//! - [`Tls`]: what is trusted, the platform's roots plus any extra ones
//!   (tests trust their fake provider's root this way).
//! - [`Identity`]: who signs, an actor and one of its device keys (§4.3).
//! - [`http`]: a provider's REST API.
//!   - [`http::Provider`] for what needs no signature: discovery (§3.1),
//!     registration and login (§4.1), registering a device key with the
//!     bootstrap token (§4.3), a user's published keys (§4.6), and anonymous
//!     reads (§4.5).
//!   - [`http::Client`] for everything signed (§4.4), one per (identity,
//!     provider). It retries a `503` with a fresh signature, follows the
//!     provider's clock, decodes problem details (§2.6), and uploads media
//!     with progress (§5.8).
//! - [`ws`]: a provider's WebSocket (§7.1): the challenge sign-in, the
//!   heartbeat, close codes, subscriptions with resume cursors, and replies
//!   matched to their commands.
//!
//! Section numbers (§) are the OFSCP v0.1 spec's (`~/dev/ofscp`).

mod clock;
mod endpoint;
mod error;
pub mod http;
mod identity;
mod tls;
mod transport;
pub mod ws;

pub use clock::ProviderClock;
pub use endpoint::{Endpoint, EndpointError};
pub use error::{Error, Problem, Result};
pub use identity::Identity;
pub use tls::{Tls, TlsError};
pub use transport::{Options, RetryPolicy, Transport};

/// The protocol crate this transport is built on, re-exported so a user
/// of this crate names the same types.
pub use ofscp_core;
