//! The command layer: every action Postrinch offers, for the window and for
//! agents alike.
//!
//! Each command takes plain arguments and returns plain results that
//! serialize, so the MCP server can expose the same commands the window
//! calls. These are the read-only commands; writing ones, and the person's
//! confirmation they need, come with sending.

mod error;
mod folders;
mod labels;
mod threads;

use postrinch_store::Store;

pub use error::{CommandError, CommandResult};
pub use folders::{AccountFolders, Folder, FolderEntry, Sidebar};
pub use labels::{AppLabels, LabelNode, LabelRole, LabelTree, app_of, role};
pub use threads::{Thread, ThreadPage};

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
