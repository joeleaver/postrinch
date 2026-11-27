//! A provider's WebSocket (§7.1): one connection per provider, carrying
//! all of that provider's channels (§8.5).
//!
//! [`connect`] opens `wss://{authority}/api/ws`, answers the provider's
//! `auth.challenge` with a signed `authenticate`, and hands back a
//! [`Connection`] to send commands on and [`Events`] to read from. A task
//! owns the socket. It:
//!
//! - answers the provider's `ping` with a `pong`, pings a quiet provider
//!   itself, and gives up on one silent past the idle timeout (§7.1
//!   "Heartbeat & liveness");
//! - matches each reply to its command by `correlationId`, so
//!   [`Connection::request`] returns the reply or the `error` (§7.3);
//! - drops a malformed frame as a local error, never answering it (spec
//!   #27: a standard type with bad `data` is not replied to);
//! - ends [`Events`] with [`Incoming::Closed`] and the [`CloseReason`],
//!   which tells a revoked key (`4003`) from one to retry (`4001`).
//!
//! Resuming is per channel (§7.1 "Resuming after a disconnect"): the
//! caller keeps each channel's last `cursor` and passes them to
//! [`Connection::subscribe`] after reconnecting; the answer lists the
//! channels whose gap was `truncated`, to backfill over REST. Delivery is
//! at least once, so the caller also de-duplicates by message id.
//! Reconnecting, and what to resubscribe, are the caller's: this is one
//! connection.

mod close;
mod connect;
mod connection;
mod driver;
#[cfg(test)]
mod tests;

use std::time::Duration;

use ofscp_core::signing::CanonicalError;
use ofscp_core::ws::{EncodeError, Envelope, Event, WsError};

pub use close::CloseReason;
pub use connect::connect;
pub use connection::{Connection, Events};

/// Timeouts and limits of a connection. The defaults follow §7.1.
#[derive(Debug, Clone)]
pub struct WsOptions {
    /// How long to wait for the challenge and the sign-in's answer.
    pub auth_timeout: Duration,
    /// How long a command waits for its reply.
    pub reply_timeout: Duration,
    /// Ping the provider after this long without hearing from it (the
    /// provider pings every 30 s, §7.1).
    pub ping_interval: Duration,
    /// Give up on a provider silent this long (the spec's 60 s, plus
    /// room for a ping to be answered).
    pub idle_timeout: Duration,
    /// The largest frame read.
    pub max_frame_bytes: usize,
    /// Events held for a reader that is behind. One more ends the
    /// connection with [`CloseReason::Overrun`], so the socket is always
    /// read and pings always answered.
    pub event_buffer: usize,
}

impl Default for WsOptions {
    fn default() -> Self {
        Self {
            auth_timeout: Duration::from_secs(10),
            reply_timeout: Duration::from_secs(30),
            ping_interval: Duration::from_secs(45),
            idle_timeout: Duration::from_secs(75),
            max_frame_bytes: 16 * 1024 * 1024,
            event_buffer: 1024,
        }
    }
}

/// What the reader of a connection gets.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// An event: a timeline change, a private message, presence, an
    /// unsolicited `unsubscribed` or `error`, and so on.
    Event(Box<Envelope<Event>>),
    /// The connection ended. Nothing follows.
    Closed(CloseReason),
}

/// Why a connection could not be opened and signed in.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// No URL for the provider.
    #[error(transparent)]
    Endpoint(#[from] crate::EndpointError),
    /// The TCP connection or the TLS handshake failed.
    #[error("cannot connect to {authority}: {source}")]
    Connect {
        /// The provider.
        authority: String,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// The WebSocket upgrade failed.
    #[error("{authority} refused the WebSocket: {source}")]
    Upgrade {
        /// The provider.
        authority: String,
        /// Why.
        #[source]
        source: Box<tokio_tungstenite::tungstenite::Error>,
    },
    /// A step took longer than allowed.
    #[error("{authority} took too long to {step}")]
    TimedOut {
        /// The provider.
        authority: String,
        /// Which step.
        step: &'static str,
    },
    /// The provider did not follow §7.1's sign-in.
    #[error("{authority} broke the sign-in: {detail}")]
    Protocol {
        /// The provider.
        authority: String,
        /// What it did.
        detail: String,
    },
    /// The provider refused the sign-in. A `4003` close means the key is
    /// revoked; see [`CloseReason::may_reconnect`].
    #[error("{authority} refused the sign-in: {close}")]
    Refused {
        /// The provider.
        authority: String,
        /// The `error` event it sent, if any.
        error: Option<Box<WsError>>,
        /// How it closed.
        close: CloseReason,
    },
    /// The sign-in could not be signed.
    #[error("cannot sign in: {0}")]
    Sign(#[from] CanonicalError),
    /// A frame could not be encoded.
    #[error("cannot encode a frame: {0}")]
    Encode(#[from] EncodeError),
}

/// Why a command got no reply.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RequestError {
    /// The connection ended first.
    #[error("{0}")]
    Closed(CloseReason),
    /// No reply within the reply timeout.
    #[error("no reply in time")]
    TimedOut,
    /// The provider answered with an `error` (§7.3).
    #[error("{} ({}): {}", .0.code, .0.status, .0.message)]
    Rejected(WsError),
    /// The reply was malformed, and dropped (spec #27).
    #[error("a malformed reply: {0}")]
    Malformed(String),
    /// A reply of another type than the command's.
    #[error("an unexpected reply: {0}")]
    Unexpected(String),
    /// The command could not be encoded.
    #[error("cannot encode the command: {0}")]
    Encode(String),
}
