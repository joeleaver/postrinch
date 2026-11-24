//! The fake's mail: each user's folders and the messages in them.
//!
//! Every change to a folder (a message added, flags changed, a message
//! expunged) takes the folder's next MODSEQ, so CONDSTORE and QRESYNC
//! clients can ask what changed since a point; expunged UIDs are kept with
//! the MODSEQ of their removal for `VANISHED (EARLIER)`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset};

/// The hierarchy delimiter of every folder name.
pub const DELIMITER: char = '/';

/// A folder's special use (RFC 6154).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialUse {
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    All,
    Flagged,
}

impl SpecialUse {
    /// The LIST attribute, as `\Sent`.
    pub fn attribute(self) -> &'static str {
        match self {
            SpecialUse::Sent => "\\Sent",
            SpecialUse::Drafts => "\\Drafts",
            SpecialUse::Trash => "\\Trash",
            SpecialUse::Junk => "\\Junk",
            SpecialUse::Archive => "\\Archive",
            SpecialUse::All => "\\All",
            SpecialUse::Flagged => "\\Flagged",
        }
    }
}

/// The system flags a client may set, in their canonical spelling.
const SYSTEM_FLAGS: [&str; 5] = ["\\Answered", "\\Flagged", "\\Deleted", "\\Seen", "\\Draft"];

/// Puts a flag in canonical form: system flags case-insensitively matched
/// and spelled as RFC 9051 does; keywords as given. `None` for a flag no
/// one may set (`\Recent`, or an unknown `\Name`).
pub(crate) fn canonical_flag(flag: &str) -> Option<String> {
    if let Some(system) = SYSTEM_FLAGS.iter().find(|f| f.eq_ignore_ascii_case(flag)) {
        return Some((*system).to_string());
    }
    (!flag.starts_with('\\') && !flag.is_empty()).then(|| flag.to_string())
}

/// The name a folder is stored under: `INBOX` is case-insensitive.
pub(crate) fn canonical_name(name: &str) -> String {
    if name.eq_ignore_ascii_case("INBOX") {
        "INBOX".to_string()
    } else {
        name.trim_end_matches(DELIMITER).to_string()
    }
}

/// One message in a folder.
#[derive(Debug, Clone)]
pub(crate) struct Message {
    pub(crate) uid: u32,
    pub(crate) flags: BTreeSet<String>,
    pub(crate) internal_date: DateTime<FixedOffset>,
    pub(crate) body: Arc<Vec<u8>>,
    pub(crate) modseq: u64,
}

/// How a flag change applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlagOp {
    Add,
    Remove,
    Replace,
}

/// One folder.
#[derive(Debug, Clone)]
pub(crate) struct Folder {
    /// Stays the same across renames, so a session knows its folder.
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) special_use: Option<SpecialUse>,
    pub(crate) selectable: bool,
    pub(crate) subscribed: bool,
    pub(crate) uid_validity: u32,
    pub(crate) uid_next: u32,
    pub(crate) highest_modseq: u64,
    /// In UID order.
    pub(crate) messages: Vec<Message>,
    /// Expunged UIDs with the MODSEQ of their expunge.
    pub(crate) vanished: Vec<(u32, u64)>,
    /// Every keyword set in this folder, spelled as it was first seen.
    /// Keywords compare case-insensitively (RFC 9051 2.3.2), so a later
    /// `$label` is the same keyword as an earlier `$Label` and is stored
    /// and reported as `$Label`.
    keywords: Vec<String>,
}

impl Folder {
    pub(crate) fn new(id: u64, name: String) -> Self {
        Self {
            id,
            name,
            special_use: None,
            selectable: true,
            subscribed: true,
            uid_validity: 1000 + u32::try_from(id).unwrap_or(u32::MAX - 1000),
            uid_next: 1,
            highest_modseq: 1,
            messages: Vec::new(),
            vanished: Vec::new(),
            keywords: Vec::new(),
        }
    }

    /// Spells each flag as this folder knows it: system flags are already
    /// canonical, and a keyword takes the spelling first seen here. With
    /// `remember`, a keyword not seen before is remembered as given;
    /// without (when flags are only being removed), it is left as is.
    fn spell(&mut self, flags: &BTreeSet<String>, remember: bool) -> BTreeSet<String> {
        let mut spelled = BTreeSet::new();
        for flag in flags {
            if flag.starts_with('\\') {
                spelled.insert(flag.clone());
                continue;
            }
            match self.keywords.iter().find(|k| k.eq_ignore_ascii_case(flag)) {
                Some(known) => {
                    spelled.insert(known.clone());
                }
                None => {
                    if remember {
                        self.keywords.push(flag.clone());
                    }
                    spelled.insert(flag.clone());
                }
            }
        }
        spelled
    }

    /// Takes the next MODSEQ.
    pub(crate) fn bump(&mut self) -> u64 {
        self.highest_modseq += 1;
        self.highest_modseq
    }

    pub(crate) fn unseen(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| !m.flags.contains("\\Seen"))
            .count()
    }

    pub(crate) fn find(&self, uid: u32) -> Option<&Message> {
        let i = self.messages.binary_search_by_key(&uid, |m| m.uid).ok()?;
        Some(&self.messages[i])
    }

    /// Adds a message with the next UID, which it returns.
    pub(crate) fn append(
        &mut self,
        body: Arc<Vec<u8>>,
        flags: BTreeSet<String>,
        internal_date: DateTime<FixedOffset>,
    ) -> u32 {
        let flags = self.spell(&flags, true);
        let uid = self.uid_next;
        self.uid_next += 1;
        let modseq = self.bump();
        self.messages.push(Message {
            uid,
            flags,
            internal_date,
            body,
            modseq,
        });
        uid
    }

    /// Changes a message's flags. `None` if there is no such message;
    /// otherwise whether anything changed (and so took a new MODSEQ).
    pub(crate) fn change_flags(
        &mut self,
        uid: u32,
        op: FlagOp,
        flags: &BTreeSet<String>,
    ) -> Option<bool> {
        let i = self.messages.binary_search_by_key(&uid, |m| m.uid).ok()?;
        let flags = &self.spell(flags, op != FlagOp::Remove);
        let current = &self.messages[i].flags;
        let next: BTreeSet<String> = match op {
            FlagOp::Add => current.union(flags).cloned().collect(),
            FlagOp::Remove => current.difference(flags).cloned().collect(),
            FlagOp::Replace => flags.clone(),
        };
        if &next == current {
            return Some(false);
        }
        let modseq = self.bump();
        let message = &mut self.messages[i];
        message.flags = next;
        message.modseq = modseq;
        Some(true)
    }

    /// Removes the messages `which` picks, returning their UIDs.
    pub(crate) fn expunge(&mut self, which: impl Fn(&Message) -> bool) -> Vec<u32> {
        let gone: Vec<u32> = self
            .messages
            .iter()
            .filter(|m| which(m))
            .map(|m| m.uid)
            .collect();
        if gone.is_empty() {
            return gone;
        }
        let modseq = self.bump();
        self.messages.retain(|m| !which(m));
        self.vanished.extend(gone.iter().map(|&uid| (uid, modseq)));
        gone
    }

    /// Gives the folder a new UIDVALIDITY, as a server does when it has
    /// lost track of its UIDs: every message is numbered again from 1 and
    /// the history of expunges is forgotten.
    pub(crate) fn renumber(&mut self, uid_validity: u32) {
        self.uid_validity = uid_validity;
        self.vanished.clear();
        let modseq = self.bump();
        for (i, message) in self.messages.iter_mut().enumerate() {
            message.uid = u32::try_from(i + 1).expect("fewer than 2^32 messages");
            message.modseq = modseq;
        }
        self.uid_next = u32::try_from(self.messages.len() + 1).expect("fewer than 2^32 messages");
    }
}

/// Why a change to the folder tree was refused, as the NO text says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeError {
    Exists,
    Missing,
    Inbox,
    HasChildren,
}

impl TreeError {
    pub(crate) fn text(&self) -> &'static str {
        match self {
            TreeError::Exists => "[ALREADYEXISTS] Mailbox already exists",
            TreeError::Missing => "[NONEXISTENT] No such mailbox",
            TreeError::Inbox => "[CANNOT] Not allowed for INBOX",
            TreeError::HasChildren => "[INUSE] Mailbox has children and no messages to delete",
        }
    }
}

/// One user's mail.
#[derive(Debug, Clone)]
pub(crate) struct Account {
    pub(crate) password: Option<String>,
    pub(crate) oauth_token: Option<String>,
    /// By canonical name.
    pub(crate) folders: BTreeMap<String, Folder>,
}

impl Account {
    pub(crate) fn folder(&self, name: &str) -> Option<&Folder> {
        self.folders.get(&canonical_name(name))
    }

    pub(crate) fn folder_mut(&mut self, name: &str) -> Option<&mut Folder> {
        self.folders.get_mut(&canonical_name(name))
    }

    pub(crate) fn folder_by_id(&self, id: u64) -> Option<&Folder> {
        self.folders.values().find(|f| f.id == id)
    }

    pub(crate) fn folder_by_id_mut(&mut self, id: u64) -> Option<&mut Folder> {
        self.folders.values_mut().find(|f| f.id == id)
    }

    fn has_children(&self, name: &str) -> bool {
        let prefix = format!("{name}{DELIMITER}");
        self.folders.keys().any(|k| k.starts_with(&prefix))
    }

    /// Creates a folder, and any missing parents, taking ids from
    /// `next_id`. A `\Noselect` folder of that name becomes selectable.
    pub(crate) fn create(&mut self, name: &str, next_id: &mut u64) -> Result<(), TreeError> {
        let name = canonical_name(name);
        if let Some(existing) = self.folders.get_mut(&name) {
            if existing.selectable {
                return Err(TreeError::Exists);
            }
            existing.selectable = true;
            return Ok(());
        }
        let mut path = String::new();
        for (i, level) in name.split(DELIMITER).enumerate() {
            if i > 0 {
                path.push(DELIMITER);
            }
            path.push_str(level);
            if !self.folders.contains_key(&path) {
                *next_id += 1;
                self.folders
                    .insert(path.clone(), Folder::new(*next_id, path.clone()));
            }
        }
        Ok(())
    }

    /// Deletes a folder (RFC 9051 6.3.4): one with children loses its
    /// messages and becomes `\Noselect` instead of going away.
    pub(crate) fn delete(&mut self, name: &str) -> Result<(), TreeError> {
        let name = canonical_name(name);
        if name == "INBOX" {
            return Err(TreeError::Inbox);
        }
        if !self.has_children(&name) {
            return self
                .folders
                .remove(&name)
                .map(|_| ())
                .ok_or(TreeError::Missing);
        }
        let folder = self.folders.get_mut(&name).ok_or(TreeError::Missing)?;
        if !folder.selectable {
            return Err(TreeError::HasChildren);
        }
        folder.expunge(|_| true);
        folder.selectable = false;
        Ok(())
    }

    /// Renames a folder and everything under it. Renaming INBOX moves its
    /// messages to a new folder and leaves INBOX empty (RFC 9051 6.3.6).
    pub(crate) fn rename(
        &mut self,
        from: &str,
        to: &str,
        next_id: &mut u64,
    ) -> Result<(), TreeError> {
        let from = canonical_name(from);
        let to = canonical_name(to);
        if !self.folders.contains_key(&from) {
            return Err(TreeError::Missing);
        }
        if self.folders.contains_key(&to) {
            return Err(TreeError::Exists);
        }
        if from == "INBOX" {
            self.create(&to, next_id)?;
            let inbox = self.folders.get_mut("INBOX").ok_or(TreeError::Missing)?;
            let moved = inbox.messages.clone();
            inbox.expunge(|_| true);
            let target = self.folders.get_mut(&to).ok_or(TreeError::Missing)?;
            for message in moved {
                target.append(message.body, message.flags, message.internal_date);
            }
            return Ok(());
        }
        let prefix = format!("{from}{DELIMITER}");
        let names: Vec<String> = self
            .folders
            .keys()
            .filter(|k| **k == from || k.starts_with(&prefix))
            .cloned()
            .collect();
        for old in names {
            let mut folder = self.folders.remove(&old).ok_or(TreeError::Missing)?;
            let new = format!("{to}{}", &old[from.len()..]);
            folder.name = new.clone();
            self.folders.insert(new, folder);
        }
        // Parents of the new name are created as a CREATE would.
        if let Some((parent, _)) = to.rsplit_once(DELIMITER)
            && !self.folders.contains_key(parent)
        {
            self.create(parent, next_id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> (Account, u64) {
        let mut account = Account {
            password: None,
            oauth_token: None,
            folders: BTreeMap::new(),
        };
        let mut next = 0;
        account.create("INBOX", &mut next).unwrap();
        (account, next)
    }

    #[test]
    fn create_makes_parents_and_rename_moves_children() {
        let (mut a, mut next) = account();
        a.create("Work/2026/Q4", &mut next).unwrap();
        assert!(a.folder("Work").is_some() && a.folder("Work/2026").is_some());
        assert_eq!(a.create("Work", &mut next), Err(TreeError::Exists));
        let id = a.folder("Work/2026").unwrap().id;
        a.rename("Work", "Jobs", &mut next).unwrap();
        assert!(a.folder("Work/2026").is_none());
        assert_eq!(a.folder("Jobs/2026").unwrap().id, id);
        assert!(a.folder("Jobs/2026/Q4").is_some());
    }

    #[test]
    fn delete_with_children_becomes_noselect() {
        let (mut a, mut next) = account();
        a.create("A/B", &mut next).unwrap();
        a.delete("A").unwrap();
        assert!(!a.folder("A").unwrap().selectable);
        assert_eq!(a.delete("A"), Err(TreeError::HasChildren));
        a.delete("A/B").unwrap();
        a.delete("A").unwrap();
        assert!(a.folder("A").is_none());
        assert_eq!(a.delete("inbox"), Err(TreeError::Inbox));
    }

    #[test]
    fn modseq_and_vanished() {
        let mut f = Folder::new(1, "X".into());
        let date = DateTime::parse_from_rfc3339("2026-11-24T10:00:00+00:00").unwrap();
        let a = f.append(Arc::new(b"a".to_vec()), BTreeSet::new(), date);
        let b = f.append(Arc::new(b"b".to_vec()), BTreeSet::new(), date);
        let seen: BTreeSet<String> = ["\\Seen".to_string()].into();
        assert_eq!(f.change_flags(a, FlagOp::Add, &seen), Some(true));
        assert_eq!(f.change_flags(a, FlagOp::Add, &seen), Some(false));
        assert_eq!(f.find(a).unwrap().modseq, f.highest_modseq);
        assert_eq!(f.expunge(|m| m.uid == b), vec![b]);
        assert_eq!(f.vanished, vec![(b, f.highest_modseq)]);
        f.renumber(77);
        assert_eq!((f.uid_validity, f.uid_next, f.messages[0].uid), (77, 2, 1));
    }

    #[test]
    fn keywords_compare_case_insensitively() {
        let mut f = Folder::new(1, "X".into());
        let date = DateTime::parse_from_rfc3339("2026-11-24T10:00:00+00:00").unwrap();
        let set =
            |names: &[&str]| -> BTreeSet<String> { names.iter().map(|n| n.to_string()).collect() };
        let a = f.append(Arc::new(b"a".to_vec()), set(&["$Label"]), date);
        let b = f.append(Arc::new(b"b".to_vec()), set(&["$LABEL", "Work"]), date);
        // The second message's keyword takes the first spelling.
        assert_eq!(f.find(b).unwrap().flags, set(&["$Label", "Work"]));
        assert_eq!(
            f.change_flags(a, FlagOp::Add, &set(&["$label"])),
            Some(false)
        );
        assert_eq!(
            f.change_flags(b, FlagOp::Remove, &set(&["work"])),
            Some(true)
        );
        assert_eq!(f.find(b).unwrap().flags, set(&["$Label"]));
        assert_eq!(
            f.change_flags(a, FlagOp::Replace, &set(&["WORK"])),
            Some(true)
        );
        assert_eq!(f.find(a).unwrap().flags, set(&["Work"]));
    }

    #[test]
    fn flags_are_canonical() {
        assert_eq!(canonical_flag("\\SEEN").as_deref(), Some("\\Seen"));
        assert_eq!(canonical_flag("$Label").as_deref(), Some("$Label"));
        assert_eq!(canonical_flag("\\Recent"), None);
    }
}
