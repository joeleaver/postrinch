//! A fake OFSCP provider for tests of Postrinch's OFSCP client.
//!
//! Each [`FakeProvider`] holds its users, groups, channels, messages and
//! conversations in memory and serves the spec's HTTP endpoints and the
//! WebSocket (§7.1) over TLS on a loopback port. Start several to test
//! federation: each verifies the others' signatures by fetching keys
//! (§4.6) as a real provider would. Every request is checked as §4.5 says
//! and recorded ([`FakeProvider::requests`]); [`Faults`] switch on the
//! failures a client has to survive.
//!
//! Clients trust the crate's test root, [`root_certificate_der`] or
//! [`root_certificate_pem`]; the certificate is valid for `127.0.0.1` and
//! `localhost`.
//!
//! # What it does not serve
//!
//! Groups and channels are seeded ([`FakeProvider::create_group`],
//! [`FakeProvider::create_channel`]), not created or changed over REST;
//! join requests are recorded and approved with
//! [`FakeProvider::add_member`]. Invites, contacts, privacy and
//! notification settings, push, webhooks, calls, discovery, guests,
//! account recovery and pre-signed uploads are not served (`404`).

mod clock;
mod faults;
mod http;
mod inner;
mod ops;
mod problem;
mod provider;
mod random;
mod record;
mod server;
mod state;
mod tls;
mod verify;
mod ws;

pub use faults::Faults;
pub use provider::FakeProvider;
pub use record::{Authentication, RecordedRequest};
pub use tls::{root_certificate_der, root_certificate_pem};
