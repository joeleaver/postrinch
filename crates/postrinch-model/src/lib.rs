//! Postrinch's domain types.
//!
//! What an account, a message, a thread, a label and a saved view *are*,
//! independent of where they are stored (`postrinch-store`) or where they
//! come from (`postrinch-sync`). Nothing here does I/O.

mod account;
mod address;
mod change;
mod draft;
mod label;
mod message;
mod thread;
mod time;
mod view;

pub use account::{Account, AccountState, Provider, SignInReason};
pub use address::{AddressError, EmailAddress, Mailbox};
pub use change::MessageChange;
pub use draft::{Answering, Draft, NotAnAddress, Proposal, format_mailboxes, parse_mailboxes};
pub use label::{Label, LabelColor, LabelKind, LabelPrefs, LabelVisibility, SystemLabel};
pub use message::{AttachmentInfo, Flags, Message, MessageHeaders, ProviderMessageId};
pub use thread::{ProviderThreadId, ThreadSummary};
pub use time::Timestamp;
pub use view::{SavedView, ViewError};
