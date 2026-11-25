//! What IMAP sync keeps about an account's folders and where each message
//! sits on the server (docs/imap.md, "Sync" and "A message's identity").

use serde::{Deserialize, Serialize};

use crate::{ProviderMessageId, SystemLabel, Timestamp};

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
    /// The copy's INTERNALDATE, when the server gave one: of a message's
    /// copies, the one that arrived first gives it its time, headers and
    /// body (docs/imap.md, "A message's identity").
    #[serde(default)]
    pub arrived: Option<Timestamp>,
}

/// A copy a change made on a server without UIDPLUS, which did not say
/// the copy's UID, and which was not found by its `Message-ID` either. It
/// stands for the copy until sync has read the folder past it, so the
/// change, sent again meanwhile, does not copy the message twice
/// (docs/imap.md, "Changing mail").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapUnplacedCopy {
    /// The label id of the folder the copy was made in.
    pub folder: String,
    /// The message the copy is of.
    pub message: ProviderMessageId,
    /// The folder's UIDVALIDITY on the server when the copy was made.
    pub uidvalidity: u32,
    /// The folder's UIDNEXT on the server just before: the copy's UID is
    /// this or above.
    pub uidnext: u32,
    /// The folder's UIDVALIDITY as sync had read it then (0 for a folder
    /// not read yet).
    pub read_uidvalidity: u32,
}

impl ImapUnplacedCopy {
    /// Whether the copy still waits for sync to find it, given the folder
    /// as sync has now read it: once sync has read the folder past the
    /// copy's UIDNEXT, at the copy's UIDVALIDITY, the copy is a place (or
    /// gone), and once the folder's UIDVALIDITY moves on to another one,
    /// sync reads the folder whole.
    pub fn waits(&self, read: &ImapFolder) -> bool {
        if read.uidvalidity == self.uidvalidity {
            return read.uidnext <= self.uidnext;
        }
        read.uidvalidity == self.read_uidvalidity
    }
}
