//! The command layer: every action Postrinch offers, for the window and for
//! agents alike.
//!
//! Each command takes plain arguments and returns plain results that
//! serialize, so the MCP server can expose the same commands the window
//! calls. Reading, and changing mail in ways that can be undone and stay in
//! the account ([`ThreadAction`]); sending, and the person's confirmation
//! it needs, come later.

mod accounts;
mod actions;
mod attachments;
mod bulk;
mod error;
mod filters;
mod folders;
mod labels;
mod messages;
mod query;
mod query_or;
mod rules;
mod rules_proposed;
mod rules_run;
mod scope;
mod search;
mod threads;
mod views;
mod writing;

use postrinch_store::Store;

pub use accounts::AccountInfo;
pub use actions::{LabelChoice, ThreadAction, ThreadRef};
pub use attachments::{file_name, is_program, mime_for, size_text};
pub use error::{CommandError, CommandResult};
pub use filters::Filters;
pub use folders::{AccountFolders, Folder, FolderEntry, Sidebar};
pub use labels::{AppLabels, Chip, LabelNode, LabelRole, LabelTree, app_of, label_term, role};
pub use messages::{Grouping, MessageRef};
pub use postrinch_render::Part;
pub use postrinch_render::Rendered;
pub use query::{LabelName, Query, label_matches};
pub use rules::{AlreadyHere, Matched, RulesPage};
pub use rules_proposed::RuleProposal;
pub use rules_run::{AgentAsk, Ran};
pub use search::{Excerpt, Remote, SearchResults};
pub use threads::{CATEGORIES, ListOrder, Thread, ThreadPage, ThreadRow};
pub use views::{EVERYTHING_ELSE, Place, ViewEntry, ViewTally};
pub use writing::{Queued, Start};

/// The commands, over one store. Cheap to clone.
#[derive(Clone)]
pub struct Commands {
    store: Store,
    /// The accounts this caller sees; `None` for all of them.
    only: scope::Only,
}

impl Commands {
    /// Commands that see every account, as the window does.
    pub fn new(store: Store) -> Self {
        Self { store, only: None }
    }

    /// Give the memory the store's reads took back to the system.
    pub fn release_memory(&self) {
        self.store.release_memory();
    }
}
