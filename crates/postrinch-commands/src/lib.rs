//! The command layer: every action Postrinch offers, for the window and for
//! agents alike.
//!
//! Each command takes plain arguments and returns plain results that
//! serialize, so the MCP server can expose the same commands the window
//! calls. These are the read-only commands; writing ones, and the person's
//! confirmation they need, come with sending.

mod accounts;
mod error;
mod folders;
mod labels;
mod search;
mod threads;

use postrinch_store::Store;

pub use accounts::AccountInfo;
pub use error::{CommandError, CommandResult};
pub use folders::{AccountFolders, Folder, FolderEntry, Sidebar};
pub use labels::{AppLabels, Chip, LabelNode, LabelRole, LabelTree, app_of, role};
pub use search::{Excerpt, SearchResults};
pub use threads::{CATEGORIES, Thread, ThreadPage, ThreadRow};

/// The commands, over one store. Cheap to clone.
#[derive(Clone)]
pub struct Commands {
    store: Store,
}

impl Commands {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}
