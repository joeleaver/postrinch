//! The fake IMAP server, driven by async-imap (the client Postrinch uses)
//! and by a line-level client for exact responses.

mod client;
mod helpers;
mod protocol;
mod sync;
