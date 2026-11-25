//! Connecting and signing in to IMAP and SMTP servers, against the fake
//! servers of `postrinch-mailtest` on loopback ports, and OAuth sign-in
//! against mock providers. No test touches a real mailbox or provider.

mod check;
mod connect;
mod misbehaving;
mod oauth;
mod oauth_grant;
mod oauth_mock;
mod oauth_robust;
mod oauth_sign_in;
mod oauth_tokens;
mod secrets;
mod smtp;
mod support;
