//! Google accounts: sign-in, and Gmail sync.

mod accounts;
mod api;
mod auth;
mod config;
mod labels;
mod oauth;
mod quota;
mod sync;

pub use accounts::{remove_google_account, sign_in_google};
pub use api::GmailApi;
pub(crate) use api::chain as error_chain;
pub use auth::GoogleAuth;
pub use config::ClientConfig;
pub(crate) use oauth::imap_spec;
pub use oauth::{Endpoints, SCOPES, SETTINGS_SCOPE};
pub use sync::{
    Answered, CalendarReport, GmailSync, KeepOptions, Phase, Progress, ProgressFn, RulesReport,
    SETTINGS_LABEL, SendReport, SyncEvent, SyncEventFn, SyncReport,
};
