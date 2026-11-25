//! The test's side of one user's mail: put messages in, look at folders,
//! and change things "from another client" while sessions are connected.
//! Every change takes a new MODSEQ and wakes IDLE sessions, as a change
//! made by another client would.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Utc};

use super::mailbox::{Account, FlagOp, Folder, Message, SpecialUse, canonical_flag};
use super::state::{Shared, State};
use crate::Error;

/// A message for the test to add to a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    body: Vec<u8>,
    flags: Vec<String>,
    internal_date: Option<DateTime<FixedOffset>>,
}

impl NewMessage {
    /// A message from its raw RFC 5322 bytes, with no flags, arriving now.
    pub fn new(body: impl Into<Vec<u8>>) -> Self {
        Self {
            body: body.into(),
            flags: Vec::new(),
            internal_date: None,
        }
    }

    /// Sets its flags, such as `\Seen` or a keyword.
    pub fn flags(mut self, flags: &[&str]) -> Self {
        self.flags = flags.iter().map(|f| f.to_string()).collect();
        self
    }

    /// Sets its INTERNALDATE.
    pub fn internal_date(mut self, date: DateTime<FixedOffset>) -> Self {
        self.internal_date = Some(date);
        self
    }
}

/// A message as the server holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageInfo {
    pub uid: u32,
    /// Flags in canonical spelling, sorted.
    pub flags: Vec<String>,
    pub internal_date: DateTime<FixedOffset>,
    pub modseq: u64,
    pub body: Vec<u8>,
}

impl MessageInfo {
    fn from(message: &Message) -> Self {
        Self {
            uid: message.uid,
            flags: message.flags.iter().cloned().collect(),
            internal_date: message.internal_date,
            modseq: message.modseq,
            body: message.body.to_vec(),
        }
    }

    /// Whether the message has `flag` (spelled canonically, as `\Seen`).
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

/// A folder as the server holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderInfo {
    pub name: String,
    pub special_use: Option<SpecialUse>,
    pub selectable: bool,
    pub subscribed: bool,
    pub uid_validity: u32,
    pub uid_next: u32,
    pub highest_modseq: u64,
    pub messages: usize,
}

impl FolderInfo {
    fn from(folder: &Folder) -> Self {
        Self {
            name: folder.name.clone(),
            special_use: folder.special_use,
            selectable: folder.selectable,
            subscribed: folder.subscribed,
            uid_validity: folder.uid_validity,
            uid_next: folder.uid_next,
            highest_modseq: folder.highest_modseq,
            messages: folder.messages.len(),
        }
    }
}

fn parse_flags(flags: &[&str]) -> Result<BTreeSet<String>, Error> {
    flags
        .iter()
        .map(|f| canonical_flag(f).ok_or_else(|| Error::InvalidFlag(f.to_string())))
        .collect()
}

/// One user's mail, from the test's side.
#[derive(Clone)]
pub struct UserHandle {
    shared: Arc<Shared>,
    user: String,
}

impl std::fmt::Debug for UserHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserHandle")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl UserHandle {
    pub(crate) fn new(shared: Arc<Shared>, user: &str) -> Self {
        Self {
            shared,
            user: user.to_string(),
        }
    }

    /// Reads the account.
    fn read<T>(&self, f: impl FnOnce(&Account) -> Result<T, Error>) -> Result<T, Error> {
        let state = self.shared.lock();
        let account = state
            .accounts
            .get(&self.user)
            .ok_or_else(|| Error::UnknownUser(self.user.clone()))?;
        f(account)
    }

    /// Changes the account, then wakes the sessions.
    fn change<T>(
        &self,
        f: impl FnOnce(&mut Account, &mut u64) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let result = {
            let mut state = self.shared.lock();
            let State {
                accounts,
                next_folder_id,
                ..
            } = &mut *state;
            let account = accounts
                .get_mut(&self.user)
                .ok_or_else(|| Error::UnknownUser(self.user.clone()))?;
            f(account, next_folder_id)?
        };
        self.shared.notify();
        Ok(result)
    }

    fn folder_mut<'a>(account: &'a mut Account, name: &str) -> Result<&'a mut Folder, Error> {
        account
            .folder_mut(name)
            .ok_or_else(|| Error::UnknownFolder(name.to_string()))
    }

    /// Creates a folder (and any missing parents).
    pub fn create_folder(&self, name: &str) -> Result<(), Error> {
        self.change(|a, next| {
            a.create(name, next)
                .map_err(|e| Error::Folder(e.text().to_string()))
        })
    }

    /// Creates a folder with a special use (`\Sent`, `\Trash`...).
    pub fn create_special_folder(&self, name: &str, role: SpecialUse) -> Result<(), Error> {
        self.create_folder(name)?;
        self.change(|a, _| {
            Self::folder_mut(a, name)?.special_use = Some(role);
            Ok(())
        })
    }

    /// Creates Sent, Drafts, Trash, Junk and Archive with their special uses.
    pub fn create_standard_folders(&self) -> Result<(), Error> {
        for (name, role) in [
            ("Sent", SpecialUse::Sent),
            ("Drafts", SpecialUse::Drafts),
            ("Trash", SpecialUse::Trash),
            ("Junk", SpecialUse::Junk),
            ("Archive", SpecialUse::Archive),
        ] {
            self.create_special_folder(name, role)?;
        }
        Ok(())
    }

    /// Creates a `\Noselect` folder: a name in the hierarchy that holds
    /// no messages and cannot be selected.
    pub fn create_noselect_folder(&self, name: &str) -> Result<(), Error> {
        self.create_folder(name)?;
        self.change(|a, _| {
            Self::folder_mut(a, name)?.selectable = false;
            Ok(())
        })
    }

    /// Deletes a folder as DELETE would.
    pub fn delete_folder(&self, name: &str) -> Result<(), Error> {
        self.change(|a, _| {
            a.delete(name)
                .map_err(|e| Error::Folder(e.text().to_string()))
        })
    }

    /// Renames a folder as RENAME would.
    pub fn rename_folder(&self, from: &str, to: &str) -> Result<(), Error> {
        self.change(|a, next| {
            a.rename(from, to, next)
                .map_err(|e| Error::Folder(e.text().to_string()))
        })
    }

    /// Hides a folder from LIST (or shows it again), as a server that
    /// briefly leaves one out of a listing does. It can still be selected.
    pub fn hide_folder(&self, name: &str, hidden: bool) -> Result<(), Error> {
        self.change(|a, _| {
            Self::folder_mut(a, name)?.listed = !hidden;
            Ok(())
        })
    }

    /// Every folder, in name order.
    pub fn folders(&self) -> Result<Vec<FolderInfo>, Error> {
        self.read(|a| Ok(a.folders.values().map(FolderInfo::from).collect()))
    }

    /// One folder.
    pub fn folder(&self, name: &str) -> Result<FolderInfo, Error> {
        self.read(|a| {
            a.folder(name)
                .map(FolderInfo::from)
                .ok_or_else(|| Error::UnknownFolder(name.to_string()))
        })
    }

    /// Adds a message to a folder; returns its UID.
    pub fn append(&self, folder: &str, message: NewMessage) -> Result<u32, Error> {
        let flags = parse_flags(&message.flags.iter().map(String::as_str).collect::<Vec<_>>())?;
        let date = message
            .internal_date
            .unwrap_or_else(|| Utc::now().fixed_offset());
        self.change(|a, _| {
            let folder = Self::folder_mut(a, folder)?;
            Ok(folder.append(Arc::new(message.body), flags, date))
        })
    }

    /// The messages in a folder, in UID order.
    pub fn messages(&self, folder: &str) -> Result<Vec<MessageInfo>, Error> {
        self.read(|a| {
            let folder = a
                .folder(folder)
                .ok_or_else(|| Error::UnknownFolder(folder.to_string()))?;
            Ok(folder.messages.iter().map(MessageInfo::from).collect())
        })
    }

    /// One message.
    pub fn message(&self, folder: &str, uid: u32) -> Result<MessageInfo, Error> {
        self.read(|a| {
            let f = a
                .folder(folder)
                .ok_or_else(|| Error::UnknownFolder(folder.to_string()))?;
            f.find(uid)
                .map(MessageInfo::from)
                .ok_or_else(|| Error::UnknownMessage(folder.to_string(), uid))
        })
    }

    fn flags(&self, folder: &str, uid: u32, op: FlagOp, flags: &[&str]) -> Result<(), Error> {
        let flags = parse_flags(flags)?;
        self.change(|a, _| {
            Self::folder_mut(a, folder)?
                .change_flags(uid, op, &flags)
                .map(|_| ())
                .ok_or_else(|| Error::UnknownMessage(folder.to_string(), uid))
        })
    }

    /// Replaces a message's flags.
    pub fn set_flags(&self, folder: &str, uid: u32, flags: &[&str]) -> Result<(), Error> {
        self.flags(folder, uid, FlagOp::Replace, flags)
    }

    /// Adds flags to a message.
    pub fn add_flags(&self, folder: &str, uid: u32, flags: &[&str]) -> Result<(), Error> {
        self.flags(folder, uid, FlagOp::Add, flags)
    }

    /// Removes flags from a message.
    pub fn remove_flags(&self, folder: &str, uid: u32, flags: &[&str]) -> Result<(), Error> {
        self.flags(folder, uid, FlagOp::Remove, flags)
    }

    /// Makes FETCH leave out a message's header and body (or give them
    /// again): it still lists, and answers for its flags, but a FETCH
    /// asking for its header or body ends `NO Some messages could not be
    /// FETCHed (Failure)`, as Gmail's does for a message it cannot read.
    pub fn withhold(&self, folder: &str, uid: u32, withheld: bool) -> Result<(), Error> {
        self.change(|a, _| {
            let folder = Self::folder_mut(a, folder)?;
            if withheld {
                folder.withheld.insert(uid);
            } else {
                folder.withheld.remove(&uid);
            }
            Ok(())
        })
    }

    /// Removes a message, as another client's expunge would.
    pub fn expunge(&self, folder: &str, uid: u32) -> Result<(), Error> {
        self.change(|a, _| {
            let gone = Self::folder_mut(a, folder)?.expunge(|m| m.uid == uid);
            if gone.is_empty() {
                Err(Error::UnknownMessage(folder.to_string(), uid))
            } else {
                Ok(())
            }
        })
    }

    /// Gives a folder a new UIDVALIDITY and numbers its messages again
    /// from 1. Sessions that have it selected are told BYE and dropped.
    pub fn set_uid_validity(&self, folder: &str, uid_validity: u32) -> Result<(), Error> {
        self.change(|a, _| {
            Self::folder_mut(a, folder)?.renumber(uid_validity);
            Ok(())
        })
    }
}
