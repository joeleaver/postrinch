//! Folders renamed by another client. A rename keeps the folder's label
//! id, so its mail, the views and rules naming it, and its state stay
//! attached; only the label's name changes.
//!
//! A folder kept from earlier passes that LIST no longer shows, and a
//! folder LIST shows that no earlier pass kept, are the same folder
//! renamed when:
//! - the server offers RFC 8474 OBJECTID and both have the same
//!   `MAILBOXID` (kept from the folder's last `SELECT`, and read for the
//!   new name by `EXAMINE`); or, failing that,
//! - they have the same UIDVALIDITY, and no other folder that went or
//!   came in this pass has it. A server keeps a folder's UIDVALIDITY
//!   across a rename (RFC 3501 6.3.5 lets it, and servers do), while a
//!   new folder takes a fresh one, so one folder gone and one come with
//!   the same UIDVALIDITY is a rename; two of either is left alone, as a
//!   guess could join the wrong ones.
//!
//! Folders whose ids differ are never joined, whatever their UIDVALIDITY.

use std::collections::HashMap;

use postrinch_model::ImapFolder;

use super::ImapSync;
use super::folder_error::FolderError;
use super::folders::syncable;
use super::wire::{Conn, Selected};
use crate::SyncResult;
use crate::imap::folders::ListedFolder;

/// New name on the server → the label id of the folder it was.
pub(super) type Renames = HashMap<String, String>;

impl ImapSync {
    /// The folders renamed since the last pass, from what LIST gave and
    /// the folders kept. Opens (`EXAMINE`) only folders new to the
    /// account, and only while a kept folder is missing.
    pub(super) async fn renames(
        &self,
        conn: &mut Conn,
        listed: &[ListedFolder],
        stored: &[ImapFolder],
    ) -> SyncResult<Renames> {
        let wanted: Vec<&ListedFolder> = listed.iter().filter(|f| syncable(f)).collect();
        let went: Vec<&ImapFolder> = stored
            .iter()
            .filter(|s| s.uidvalidity != 0 && !wanted.iter().any(|f| f.name == s.name))
            .collect();
        if went.is_empty() {
            return Ok(Renames::new());
        }
        let mut came = Vec::new();
        for folder in wanted {
            if stored.iter().any(|s| s.name == folder.name) {
                continue;
            }
            match conn.examine(&folder.name).await {
                Ok(Some(selected)) => came.push((folder.name.clone(), selected)),
                // Not opened, it is matched with none: a folder of its
                // own, read whole, as one that refuses EXAMINE.
                Ok(None) => {}
                Err(FolderError::Busy(message)) => tracing::info!(
                    "IMAP: {} is busy for the folder {:?}: {message}",
                    conn.describe(),
                    folder.name
                ),
                Err(FolderError::Sync(e)) => return Err(e),
            }
        }
        Ok(match_renames(&went, &came))
    }
}

/// Which folders that came are folders that went (see the module).
fn match_renames(went: &[&ImapFolder], came: &[(String, Selected)]) -> Renames {
    let mut renames = Renames::new();
    let differ = |old: &ImapFolder, new: &Selected| matches!((&old.mailbox_id, &new.mailbox_id), (Some(a), Some(b)) if a != b);
    for (name, new) in came {
        let by_id = new
            .mailbox_id
            .as_ref()
            .and_then(|id| went.iter().find(|old| old.mailbox_id.as_ref() == Some(id)));
        let by_uidvalidity = || {
            let same = |uv: u32| uv != 0 && uv == new.uidvalidity;
            let old: Vec<&&ImapFolder> = went
                .iter()
                .filter(|old| same(old.uidvalidity) && !differ(old, new))
                .collect();
            let others = came
                .iter()
                .filter(|(_, other)| same(other.uidvalidity))
                .count();
            (old.len() == 1 && others == 1).then(|| old[0])
        };
        if let Some(old) = by_id.or_else(by_uidvalidity) {
            renames.insert(name.clone(), old.label_id.clone());
        }
    }
    // One folder that went is one folder renamed, at most.
    let mut seen = HashMap::new();
    for (name, id) in &renames {
        seen.entry(id.clone())
            .or_insert_with(Vec::new)
            .push(name.clone());
    }
    renames.retain(|_, id| seen.get(id).is_some_and(|names| names.len() == 1));
    renames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(label: &str, name: &str, uidvalidity: u32, id: Option<&str>) -> ImapFolder {
        let mut f = ImapFolder::new(label, name);
        f.uidvalidity = uidvalidity;
        f.mailbox_id = id.map(str::to_owned);
        f
    }

    fn opened(name: &str, uidvalidity: u32, id: Option<&str>) -> (String, Selected) {
        let selected = Selected {
            uidvalidity,
            uidnext: None,
            highest_modseq: None,
            exists: 0,
            mailbox_id: id.map(str::to_owned),
        };
        (name.to_owned(), selected)
    }

    #[test]
    fn the_same_mailbox_id_is_a_rename() {
        let work = kept("f1", "Work", 7, Some("F1"));
        let found = match_renames(&[&work], &[opened("Projects", 9, Some("F1"))]);
        assert_eq!(found.get("Projects").map(String::as_str), Some("f1"));
    }

    #[test]
    fn different_mailbox_ids_are_never_joined() {
        let work = kept("f1", "Work", 7, Some("F1"));
        let found = match_renames(&[&work], &[opened("Projects", 7, Some("F2"))]);
        assert!(found.is_empty());
    }

    #[test]
    fn one_gone_and_one_come_with_the_same_uidvalidity_is_a_rename() {
        let work = kept("f1", "Work", 7, None);
        let other = kept("f2", "Old", 8, None);
        let found = match_renames(&[&work, &other], &[opened("Projects", 7, None)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found["Projects"], "f1");
    }

    #[test]
    fn an_ambiguous_uidvalidity_is_left_alone() {
        let a = kept("f1", "A", 7, None);
        let b = kept("f2", "B", 7, None);
        assert!(match_renames(&[&a, &b], &[opened("C", 7, None)]).is_empty());
        let a = kept("f1", "A", 7, None);
        let came = [opened("C", 7, None), opened("D", 7, None)];
        assert!(match_renames(&[&a], &came).is_empty());
    }
}
