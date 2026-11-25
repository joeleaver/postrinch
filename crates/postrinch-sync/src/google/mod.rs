//! Google accounts: sign-in, and Gmail sync.

mod accounts;
mod api;
mod auth;
mod config;
mod grant;
mod labels;
mod ledger;
mod oauth;
mod quota;
mod sync;

pub use accounts::{remove_google_account, sign_in_google};
pub use api::GmailApi;
pub(crate) use api::chain as error_chain;
pub use auth::GoogleAuth;
pub use config::ClientConfig;
pub(crate) use grant::GoogleGrants;
pub(crate) use ledger::{GrantLedger, UnderWay};
pub(crate) use oauth::imap_spec;
pub use oauth::{Endpoints, SCOPES, SETTINGS_SCOPE};
pub use sync::{Answered, CalendarReport, GmailSync, RulesReport, SETTINGS_LABEL};
