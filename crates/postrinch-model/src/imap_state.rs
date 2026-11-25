//! What IMAP sync keeps about an account's folders and where each message
//! sits on the server (docs/imap.md, "Sync" and "A message's identity").

use serde::{Deserialize, Serialize};

use crate::{ProviderMessageId, SystemLabel};

/// The special role a folder plays, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FolderRole {
    /// The inbox, Sent, Drafts, Trash or Junk (`SystemLabel::Spam`): the
    /// folder's label is that system label.
    System(SystemLabel),
    /// Where archiving moves mail (`\Archive`). Its label is an ordinary
    /// one, so archived mail shows in All mail as in Gmail.
    Archive,
}

/// One folder of an account as sync knows it: the label it is shown as,
/// its name on the server and how far sync has read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapFolder {
    /// The id of the label it is: a system label's own id, or one minted
    /// for it (`f` and 12 hex digits) and kept for the folder's life.
    pub label_id: String,
    /// The folder's full name as the server spells it (modified UTF-7,
    /// with the server's delimiter).
    pub name: String,
    /// The server's hierarchy delimiter; `None` for a flat namespace.
    pub delimiter: Option<char>,
    pub role: Option<FolderRole>,
    /// The folder's UIDVALIDITY when it was last read; 0 before the first
    /// read, so any the server gives differs.
    pub uidvalidity: u32,
    /// The UIDNEXT up to which the folder has been read: every message
    /// below it is stored.
    pub uidnext: u32,
    /// The HIGHESTMODSEQ its flags were last read at (CONDSTORE); 0 for
    /// none.
    pub highest_modseq: u64,
    pub subscribed: bool,
    /// The folder's RFC 8474 `MAILBOXID` (OBJECTID), which stays the same
    /// when the folder is renamed; `None` where the server has none.
    #[serde(default)]
    pub mailbox_id: Option<String>,
}

impl ImapFolder {
    /// A folder not read yet.
    pub fn new(label_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            label_id: label_id.into(),
            name: name.into(),
            delimiter: None,
            role: None,
            uidvalidity: 0,
            uidnext: 0,
            highest_modseq: 0,
            subscribed: true,
            mailbox_id: None,
        }
    }
}

/// One copy of a message on the server: a UID in a folder. A message has
/// one place for each folder holding a copy of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapPlace {
    /// The label id of the folder the copy is in.
    pub folder: String,
    /// The folder's UIDVALIDITY the UID belongs to.
    pub uidvalidity: u32,
    pub uid: u32,
    /// The message the copy is of.
    pub message: ProviderMessageId,
    /// The copy's flags as the server has them (`\Seen`, `\Flagged`,
    /// keywords), as the server spells them.
    pub flags: Vec<String>,
    /// The copy's MODSEQ (CONDSTORE); 0 for none.
    pub modseq: u64,
}
