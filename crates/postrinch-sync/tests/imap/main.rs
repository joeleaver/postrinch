//! Connecting and signing in to IMAP and SMTP servers, against the fake
//! servers of `postrinch-mailtest` on loopback ports. No test touches a
//! real mailbox.

mod check;
mod connect;
mod misbehaving;
mod oauth;
mod secrets;
mod smtp;
mod support;
