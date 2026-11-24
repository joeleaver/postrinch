//! IMAP and SMTP accounts (docs/imap.md). So far, the pure parts every
//! later piece builds on: how a message, a thread and a user folder are
//! named.

mod ids;
mod threading;

pub use ids::{message_id, message_id_without_header, mint_folder_label_id};
pub use threading::thread_for;
