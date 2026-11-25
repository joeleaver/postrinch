//! OAuth 2 for IMAP accounts (docs/imap.md, "Credentials"): Google over
//! IMAP and Microsoft. Which provider an address uses comes from its
//! discovery ([`OAuthProvider::for_discovery`]); the clients from
//! Postrinch's config directory ([`OAuthClients`]); sign-in and access
//! tokens from [`ImapOAuth`], which is the IMAP accounts'
//! [`AccessTokens`](super::AccessTokens).

mod access;
mod clients;
mod grant;
mod microsoft;
mod provider;
mod sign_in;

pub use access::ImapOAuth;
pub use clients::OAuthClients;
pub use microsoft::MicrosoftClient;
pub use provider::OAuthProvider;
pub use sign_in::OAuthSignIn;
