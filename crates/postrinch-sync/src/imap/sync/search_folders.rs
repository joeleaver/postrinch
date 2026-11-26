//! Which folders a server search looks in, and what it asks in each.
//!
//! An alternative naming folders (`in:`, `label:`) looks in those. One
//! that names none looks where a search covers: every folder but Trash
//! and Junk (they too with `in:trash`, `in:spam` or `in:anywhere`), or on
//! Gmail's servers its All Mail, which holds every message but those two
//! (docs/imap.md, "Searching the server"). All Mail is known by LIST's
//! `\All` ([`super::folders::holds_everything`]), never by the archive
//! role: the person may make another folder the archive. Folders it
//! leaves out (`-in:`, `-label:`) are not looked in; a message with copies
//! in other folders too is checked by its labels after
//! (`postrinch-commands`).
//!
//! The alternatives looking in one folder are asked at once, joined by
//! `OR`, so a folder is opened and searched once.

use std::collections::HashSet;

use postrinch_model::{
    FolderRole, ImapAlternative, ImapFolder, ImapSearch, SearchKey, SystemLabel,
};

use super::folders::rank;
use super::names::Command;
use super::search_keys::{gmail_raw, uid_search};

/// What to ask in one folder.
#[derive(Debug, Clone)]
pub(super) struct FolderSearch {
    pub folder: ImapFolder,
    pub command: Command,
}

/// Each folder to look in, the inbox first and Trash and Junk last, with
/// the command that asks there what every alternative looking in it asks.
/// `gmail`: the server is Gmail's, so Gmail's language (`X-GM-RAW`) is
/// how it is asked. `everything`: the label ids of the folders that hold
/// every message (Gmail's All Mail), where a search covers when there is
/// one.
pub(super) fn plan(
    folders: &[ImapFolder],
    search: &ImapSearch,
    gmail: bool,
    everything: &HashSet<String>,
) -> Vec<FolderSearch> {
    let mut ordered: Vec<&ImapFolder> = folders.iter().collect();
    ordered.sort_by_key(|f| rank(f.role));
    let mut out = Vec::new();
    for folder in ordered {
        let asking: Vec<&ImapAlternative> = search
            .alternatives
            .iter()
            .filter(|a| looks_in(a, folder, folders, everything))
            .collect();
        if asking.is_empty() {
            continue;
        }
        // An alternative with nothing in Gmail's language to ask (only
        // what is left out, or `last:`) asks for every message there.
        let every = asking.iter().any(|a| a.gmail.trim().is_empty());
        let command = if gmail && every {
            uid_search(&SearchKey::All(Vec::new()))
        } else if gmail {
            let texts: Vec<&str> = asking.iter().map(|a| a.gmail.as_str()).collect();
            gmail_raw(&match texts.as_slice() {
                [one] => (*one).to_owned(),
                many => many
                    .iter()
                    .map(|t| format!("({t})"))
                    .collect::<Vec<_>>()
                    .join(" OR "),
            })
        } else {
            let key = SearchKey::any(asking.iter().map(|a| a.key.clone()))
                .unwrap_or(SearchKey::All(Vec::new()));
            uid_search(&key)
        };
        out.push(FolderSearch {
            folder: folder.clone(),
            command,
        });
    }
    out
}

/// Whether an alternative looks in `folder`, one of the account's
/// `folders`, of which those in `everything` hold every message.
fn looks_in(
    alternative: &ImapAlternative,
    folder: &ImapFolder,
    folders: &[ImapFolder],
    everything: &HashSet<String>,
) -> bool {
    if alternative.not_in.contains(&folder.label_id) {
        return false;
    }
    if !alternative.folders.is_empty() {
        return alternative.folders.contains(&folder.label_id);
    }
    let set_aside = matches!(
        folder.role,
        Some(FolderRole::System(SystemLabel::Trash | SystemLabel::Spam))
    );
    if set_aside {
        return alternative.set_aside_too;
    }
    // Gmail's All Mail holds every other folder's mail, whichever folder
    // is the archive.
    let all_mail = folders.iter().any(|f| everything.contains(&f.label_id));
    !all_mail || everything.contains(&folder.label_id)
}

/// Whether some alternative asks what only Gmail's language can, and
/// what, for a server without it.
pub(super) fn inexact(search: &ImapSearch) -> Option<&str> {
    search
        .alternatives
        .iter()
        .find_map(|a| a.inexact.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(label: &str, role: Option<FolderRole>) -> ImapFolder {
        let mut f = ImapFolder::new(label, label);
        f.role = role;
        f
    }

    fn account(gmail: bool) -> Vec<ImapFolder> {
        let system = |s: SystemLabel| folder(s.provider_id(), Some(FolderRole::System(s)));
        let mut folders = vec![
            system(SystemLabel::Trash),
            folder("fwork", None),
            system(SystemLabel::Spam),
            system(SystemLabel::Inbox),
            system(SystemLabel::Sent),
        ];
        if gmail {
            folders.push(folder("fall", Some(FolderRole::Archive)));
        }
        folders
    }

    fn alternative(text: &str) -> ImapAlternative {
        ImapAlternative {
            folders: Vec::new(),
            not_in: Vec::new(),
            set_aside_too: false,
            key: SearchKey::Text(text.into()),
            gmail: text.into(),
            inexact: None,
        }
    }

    fn searched(plan: &[FolderSearch]) -> Vec<(String, String)> {
        plan.iter()
            .map(|p| {
                let first = p.command.wire(true, false).first;
                (p.folder.label_id.clone(), first)
            })
            .collect()
    }

    fn search(alternatives: Vec<ImapAlternative>) -> ImapSearch {
        ImapSearch { alternatives }
    }

    /// The plan, with `fall` as Gmail's All Mail where the account has it
    /// and the server is Gmail's.
    fn plan_for(folders: &[ImapFolder], search: &ImapSearch, gmail: bool) -> Vec<FolderSearch> {
        let everything = folders
            .iter()
            .filter(|f| gmail && f.label_id == "fall")
            .map(|f| f.label_id.clone())
            .collect();
        plan(folders, search, gmail, &everything)
    }

    #[test]
    fn a_search_covers_every_folder_but_trash_and_junk() {
        let planned = plan_for(&account(false), &search(vec![alternative("x")]), false);
        let folders: Vec<String> = searched(&planned).into_iter().map(|(f, _)| f).collect();
        assert_eq!(folders, ["INBOX", "SENT", "fwork"]);
        assert!(
            searched(&planned)
                .iter()
                .all(|(_, c)| c == r#"UID SEARCH TEXT "x""#)
        );
    }

    #[test]
    fn set_aside_folders_when_asked() {
        let mut anywhere = alternative("x");
        anywhere.set_aside_too = true;
        let planned = plan_for(&account(false), &search(vec![anywhere]), false);
        let folders: Vec<String> = searched(&planned).into_iter().map(|(f, _)| f).collect();
        assert_eq!(folders, ["INBOX", "SENT", "fwork", "TRASH", "SPAM"]);
    }

    #[test]
    fn named_folders_and_folders_left_out() {
        let mut work = alternative("x");
        work.folders = vec!["fwork".into()];
        let mut not_sent = alternative("y");
        not_sent.not_in = vec!["SENT".into()];
        let planned = plan_for(&account(false), &search(vec![work, not_sent]), false);
        assert_eq!(
            searched(&planned),
            [
                ("INBOX".to_owned(), r#"UID SEARCH TEXT "y""#.to_owned()),
                (
                    "fwork".to_owned(),
                    r#"UID SEARCH OR TEXT "x" TEXT "y""#.to_owned()
                ),
            ]
        );
        let mut trash = alternative("z");
        trash.folders = vec!["TRASH".into()];
        trash.set_aside_too = true;
        let planned = plan_for(&account(false), &search(vec![trash]), false);
        assert_eq!(searched(&planned)[0].0, "TRASH");
    }

    #[test]
    fn gmail_looks_in_all_mail_in_its_own_language() {
        let mut work = alternative("label:work y");
        work.folders = vec!["fwork".into()];
        let planned = plan_for(
            &account(true),
            &search(vec![
                alternative("x"),
                alternative("has:attachment z"),
                work,
            ]),
            true,
        );
        assert_eq!(
            searched(&planned),
            [
                (
                    "fwork".to_owned(),
                    r#"UID SEARCH X-GM-RAW "label:work y""#.to_owned()
                ),
                (
                    "fall".to_owned(),
                    r#"UID SEARCH X-GM-RAW "(x) OR (has:attachment z)""#.to_owned()
                ),
            ]
        );
    }

    #[test]
    fn gmail_looks_in_all_mail_whichever_folder_archives() {
        // The person made `fwork` the archive: All Mail stays where a
        // search covers, and the archive is no reason to look there.
        let mut folders = account(true);
        for f in &mut folders {
            f.role = match f.label_id.as_str() {
                "fall" => None,
                "fwork" => Some(FolderRole::Archive),
                _ => f.role,
            };
        }
        let planned = plan_for(&folders, &search(vec![alternative("x")]), true);
        assert_eq!(
            searched(&planned),
            [("fall".to_owned(), r#"UID SEARCH X-GM-RAW "x""#.to_owned())]
        );
    }

    #[test]
    fn gmail_with_nothing_to_ask_asks_for_everything() {
        // `-from:jose`: what is left out is left to the check.
        let mut left_out = alternative("");
        left_out.key = SearchKey::All(Vec::new());
        let planned = plan_for(
            &account(true),
            &search(vec![alternative("x"), left_out]),
            true,
        );
        assert_eq!(
            searched(&planned),
            [("fall".to_owned(), "UID SEARCH ALL".to_owned())]
        );
    }

    #[test]
    fn gmail_without_all_mail_looks_everywhere() {
        let planned = plan_for(&account(false), &search(vec![alternative("x")]), true);
        assert_eq!(planned.len(), 3);
    }

    #[test]
    fn nothing_to_ask_looks_nowhere() {
        assert!(plan_for(&account(false), &search(Vec::new()), false).is_empty());
        let mut inexact = alternative("x");
        assert_eq!(super::inexact(&search(vec![inexact.clone()])), None);
        inexact.inexact = Some("has:attachment".into());
        assert_eq!(
            super::inexact(&search(vec![alternative("y"), inexact])),
            Some("has:attachment")
        );
    }
}
