//! The person's labels as the sidebar shows them: a tree, with what other
//! mail apps left behind set apart, and sensible defaults for what shows.
//!
//! Anyone who has used a few mail apps has labels they never made: Superhuman
//! files its AI sorting under `[Superhuman]/…`, Apple Mail and Outlook over
//! IMAP leave `[Imap]/Sent`, `Deleted Items` and `Notes`, SaneBox adds
//! `@SaneLater`. Those go under the app that made them. Labels Gmail hides,
//! labels with nothing in them and show-if-unread labels with nothing unread
//! are hidden until the person asks for all of them. Whether a label works
//! as a folder or a tag is read from how its mail is filed.

use std::collections::HashMap;

use postrinch_model::{EmailAddress, Label, LabelKind, LabelPrefs, LabelVisibility};
use postrinch_store::LabelStats;
use serde::{Deserialize, Serialize};

use crate::{Folder, FolderEntry};

/// How a label is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelRole {
    /// Its mail is filed there and nowhere else, out of the inbox.
    Folder,
    /// It marks mail that lives elsewhere too.
    Tag,
}

/// One level of the label tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelNode {
    /// This level's part of the provider's name: `Clients` in
    /// `Work/Clients`.
    pub segment: String,
    /// What this level is called: its segment, or the person's own name.
    pub name: String,
    /// The label at this level; `None` where a name has parts no label of
    /// its own stands for (`Work` in `Work/Clients` when there is no `Work`).
    pub entry: Option<FolderEntry>,
    pub role: LabelRole,
    /// A background colour, `#rrggbb`: the person's, or the provider's.
    pub color: Option<String>,
    /// Whether it shows unless the person asks for all labels.
    pub shown: bool,
    pub messages: usize,
    /// What the person changed about the label here.
    pub prefs: LabelPrefs,
    pub children: Vec<LabelNode>,
}

/// The labels one other app made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppLabels {
    pub app: String,
    /// The app's own top label (`[Superhuman]`), when there is one.
    pub entry: Option<FolderEntry>,
    pub labels: Vec<LabelNode>,
    pub shown: bool,
}

/// An account's labels, sorted out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelTree {
    /// Labels the person pinned, listed with the folders at the top.
    pub pinned: Vec<FolderEntry>,
    /// The person's own labels.
    pub own: Vec<LabelNode>,
    /// Labels other mail apps made, by app.
    pub other_apps: Vec<AppLabels>,
}

impl LabelTree {
    /// Every label in the tree, with a label of its own, in tree order.
    pub fn entries(&self) -> Vec<&FolderEntry> {
        fn walk<'a>(nodes: &'a [LabelNode], out: &mut Vec<&'a FolderEntry>) {
            for node in nodes {
                out.extend(node.entry.as_ref());
                walk(&node.children, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.own, &mut out);
        for app in &self.other_apps {
            out.extend(app.entry.as_ref());
            walk(&app.labels, &mut out);
        }
        out
    }
}

/// A label as a thread row shows it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chip {
    pub label: String,
    pub name: String,
    pub color: Option<String>,
}

/// The chips an account's labels show as on thread rows, by provider id:
/// labels that work as tags and show in the sidebar. A folder-like label
/// is where the mail lives, which the folder list already says.
pub(crate) fn chips(
    labels: &[Label],
    prefs: &HashMap<String, LabelPrefs>,
    stats: &HashMap<String, LabelStats>,
) -> HashMap<String, Chip> {
    let mut chips = HashMap::new();
    for label in labels.iter().filter(|l| l.kind == LabelKind::User) {
        let prefs = prefs.get(&label.provider_id).cloned().unwrap_or_default();
        let stats = stats.get(&label.provider_id).copied().unwrap_or_default();
        let shown = prefs
            .shown
            .unwrap_or_else(|| shown_by_default(label, &stats));
        if !shown || role(&stats) == LabelRole::Folder {
            continue;
        }
        let name = prefs
            .name
            .clone()
            .unwrap_or_else(|| match app_of(&label.name) {
                Some((_, rest)) if !rest.is_empty() => rest,
                _ => label.name.clone(),
            });
        let color = prefs
            .color
            .clone()
            .or_else(|| label.color.as_ref().map(|c| c.background.clone()));
        chips.insert(
            label.provider_id.clone(),
            Chip {
                label: label.provider_id.clone(),
                name,
                color,
            },
        );
    }
    chips
}

/// Names that IMAP mail apps (Outlook, Apple Mail, Thunderbird) create for
/// their own folders, when a Gmail account is added to them over IMAP.
const IMAP_CLIENT_FOLDERS: [&str; 11] = [
    "Conversation History",
    "Deleted Items",
    "Deleted Messages",
    "Junk",
    "Junk E-mail",
    "Junk Email",
    "Notes",
    "Outbox",
    "RSS Feeds",
    "Sent Items",
    "Sync Issues",
];

/// The app a label belongs to, and its name within that app: `None` for a
/// label the person made.
pub fn app_of(name: &str) -> Option<(String, String)> {
    if let Some(inner) = name.strip_prefix('[')
        && let Some(close) = inner.find(']')
    {
        let app = match &inner[..close] {
            "Imap" | "IMAP" => "Mail apps over IMAP".to_owned(),
            "Gmail" | "Google Mail" => "Gmail over IMAP".to_owned(),
            other => other.to_owned(),
        };
        let rest = inner[close + 1..].trim_start_matches('/').to_owned();
        return Some((app, rest));
    }
    if name.starts_with("@Sane") {
        return Some(("SaneBox".to_owned(), name.to_owned()));
    }
    let top = name.split('/').next().unwrap_or(name);
    IMAP_CLIENT_FOLDERS
        .contains(&top)
        .then(|| ("Mail apps over IMAP".to_owned(), name.to_owned()))
}

/// Whether a label works as a folder: nearly none of its mail is in the
/// inbox, and most of it is filed under this label alone.
pub fn role(stats: &LabelStats) -> LabelRole {
    let n = stats.messages;
    if n > 0 && stats.in_inbox * 10 <= n && stats.filed_only_here * 4 >= n * 3 {
        LabelRole::Folder
    } else {
        LabelRole::Tag
    }
}

/// Whether a label shows by default.
fn shown_by_default(label: &Label, stats: &LabelStats) -> bool {
    match label.visibility {
        LabelVisibility::Hide => false,
        LabelVisibility::ShowIfUnread => stats.unread > 0,
        LabelVisibility::Show => stats.messages > 0,
    }
}

/// Sort an account's own labels into a tree. System labels are left out:
/// the sidebar has its own places for them.
pub fn label_tree(
    account: &EmailAddress,
    labels: &[Label],
    prefs: &HashMap<String, LabelPrefs>,
    stats: &HashMap<String, LabelStats>,
) -> LabelTree {
    let mut tree = LabelTree::default();
    let mut apps: Vec<AppLabels> = Vec::new();
    for label in labels.iter().filter(|l| l.kind == LabelKind::User) {
        let prefs = prefs.get(&label.provider_id).cloned().unwrap_or_default();
        let stats = stats.get(&label.provider_id).copied().unwrap_or_default();
        let entry = FolderEntry {
            folder: Folder::Label {
                account: account.clone(),
                label: label.provider_id.clone(),
            },
            name: prefs.name.clone().unwrap_or_else(|| label.name.clone()),
            unread: stats.unread,
        };
        if prefs.pinned {
            tree.pinned.push(entry.clone());
        }
        let leaf = LabelNode {
            segment: String::new(),
            name: prefs.name.clone().unwrap_or_default(),
            entry: Some(entry.clone()),
            role: role(&stats),
            color: prefs
                .color
                .clone()
                .or_else(|| label.color.as_ref().map(|c| c.background.clone())),
            shown: prefs
                .shown
                .unwrap_or_else(|| shown_by_default(label, &stats)),
            messages: stats.messages,
            prefs: prefs.clone(),
            children: Vec::new(),
        };
        match app_of(&label.name) {
            None => insert(&mut tree.own, &path(&label.name), leaf),
            Some((app, rest)) => {
                let group = match apps.iter().position(|a| a.app == app) {
                    Some(i) => &mut apps[i],
                    None => {
                        apps.push(AppLabels {
                            app,
                            entry: None,
                            labels: Vec::new(),
                            shown: false,
                        });
                        apps.last_mut().expect("just pushed")
                    }
                };
                if rest.is_empty() {
                    group.entry = Some(entry);
                } else {
                    insert(&mut group.labels, &path(&rest), leaf);
                }
            }
        }
    }
    finish(&mut tree.own);
    for group in &mut apps {
        finish(&mut group.labels);
        group.shown = group.labels.iter().any(|n| n.shown);
    }
    apps.sort_by_key(|a| a.app.to_lowercase());
    tree.other_apps = apps;
    tree.pinned.sort_by_key(|e| e.name.to_lowercase());
    tree
}

fn path(name: &str) -> Vec<&str> {
    name.split('/').filter(|s| !s.is_empty()).collect()
}

/// Put `leaf` at `path`, making the levels above it as needed.
fn insert(nodes: &mut Vec<LabelNode>, path: &[&str], mut leaf: LabelNode) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    let i = match nodes.iter().position(|n| n.segment == *first) {
        Some(i) => i,
        None => {
            nodes.push(LabelNode {
                segment: (*first).to_owned(),
                name: (*first).to_owned(),
                entry: None,
                role: LabelRole::Tag,
                color: None,
                shown: false,
                messages: 0,
                prefs: LabelPrefs::default(),
                children: Vec::new(),
            });
            nodes.len() - 1
        }
    };
    let node = &mut nodes[i];
    if rest.is_empty() {
        // The label for this level: keep any children already placed.
        leaf.segment = (*first).to_owned();
        if leaf.name.is_empty() {
            leaf.name = (*first).to_owned();
        }
        leaf.children = std::mem::take(&mut node.children);
        *node = leaf;
    } else {
        insert(&mut node.children, rest, leaf);
    }
}

/// Sort each level by name, and show a level whenever something under it
/// shows.
fn finish(nodes: &mut [LabelNode]) {
    nodes.sort_by_key(|n| n.name.to_lowercase());
    for node in nodes.iter_mut() {
        finish(&mut node.children);
        node.shown = node.shown || node.children.iter().any(|c| c.shown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> EmailAddress {
        EmailAddress::parse("me@example.org").unwrap()
    }

    fn counts(messages: usize, unread: usize, in_inbox: usize, only_here: usize) -> LabelStats {
        LabelStats {
            messages,
            unread,
            in_inbox,
            filed_only_here: only_here,
        }
    }

    /// Labels like a real account that has seen Superhuman, Zoom Mail and
    /// Outlook over IMAP.
    fn account() -> (Vec<Label>, HashMap<String, LabelStats>) {
        let user = |id: &str, name: &str| Label::new(id, name, LabelKind::User);
        let mut hidden = user("L9", "[Superhuman]/Muted");
        hidden.visibility = LabelVisibility::Hide;
        let labels = vec![
            Label::new(
                "INBOX",
                "INBOX",
                LabelKind::System(postrinch_model::SystemLabel::Inbox),
            ),
            user("L1", "Authnet Auto-Receipt"),
            user("L2", "Work/Clients"),
            user("L3", "[Superhuman]"),
            user("L4", "[Superhuman]/AI/Marketing"),
            user("L5", "[Superhuman]/AI/Social"),
            user("L6", "[Zoom Mail]/Reply Later"),
            user("L7", "Deleted Items"),
            user("L8", "@SaneLater"),
            hidden,
        ];
        let stats = HashMap::from([
            ("L1".into(), counts(742, 23, 0, 742)),
            ("L2".into(), counts(10, 2, 8, 0)),
            ("L4".into(), counts(94, 70, 0, 31)),
            ("L7".into(), counts(3, 0, 0, 3)),
            ("L8".into(), counts(5, 5, 0, 5)),
            ("L9".into(), counts(4, 0, 0, 0)),
        ]);
        (labels, stats)
    }

    #[test]
    fn sets_other_apps_labels_apart() {
        let (labels, stats) = account();
        let tree = label_tree(&me(), &labels, &HashMap::new(), &stats);

        let own: Vec<_> = tree.own.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(own, ["Authnet Auto-Receipt", "Work"]);
        let apps: Vec<_> = tree.other_apps.iter().map(|a| a.app.as_str()).collect();
        assert_eq!(
            apps,
            ["Mail apps over IMAP", "SaneBox", "Superhuman", "Zoom Mail"]
        );

        let superhuman = &tree.other_apps[2];
        assert!(
            superhuman.entry.is_some(),
            "[Superhuman] itself is the group's label"
        );
        assert_eq!(superhuman.labels[0].name, "AI");
        let ai: Vec<_> = superhuman.labels[0]
            .children
            .iter()
            .map(|n| (n.name.as_str(), n.shown))
            .collect();
        assert_eq!(ai, [("Marketing", true), ("Social", false)]);
        assert!(superhuman.shown);
        // Empty, so hidden: Zoom Mail's whole group.
        assert!(!tree.other_apps[3].shown);
    }

    #[test]
    fn nests_and_reads_folders_from_tags() {
        let (labels, stats) = account();
        let tree = label_tree(&me(), &labels, &HashMap::new(), &stats);
        let authnet = &tree.own[0];
        assert_eq!(authnet.role, LabelRole::Folder);
        let work = &tree.own[1];
        assert!(work.entry.is_none(), "no label is called Work by itself");
        assert!(work.shown, "a level shows when something under it does");
        assert_eq!(work.children[0].name, "Clients");
        assert_eq!(work.children[0].role, LabelRole::Tag);
        assert_eq!(work.children[0].entry.as_ref().unwrap().unread, 2);
    }

    #[test]
    fn hides_what_gmail_hides_and_what_is_empty() {
        let (labels, stats) = account();
        let tree = label_tree(&me(), &labels, &HashMap::new(), &stats);
        let superhuman = &tree.other_apps[2];
        let muted = superhuman
            .labels
            .iter()
            .find(|n| n.name == "Muted")
            .unwrap();
        assert!(!muted.shown, "Gmail hides it");

        let mut later = Label::new("L10", "Later", LabelKind::User);
        later.visibility = LabelVisibility::ShowIfUnread;
        let quiet = HashMap::from([("L10".to_owned(), counts(3, 0, 0, 3))]);
        let tree = label_tree(&me(), &[later.clone()], &HashMap::new(), &quiet);
        assert!(!tree.own[0].shown);
        let busy = HashMap::from([("L10".to_owned(), counts(3, 1, 0, 3))]);
        assert!(label_tree(&me(), &[later], &HashMap::new(), &busy).own[0].shown);
    }

    #[test]
    fn the_persons_prefs_win() {
        let (labels, stats) = account();
        let prefs = HashMap::from([
            (
                "L1".to_owned(),
                LabelPrefs {
                    name: Some("Receipts".into()),
                    pinned: true,
                    color: Some("#2456b8".into()),
                    ..Default::default()
                },
            ),
            (
                "L5".to_owned(),
                LabelPrefs {
                    shown: Some(true),
                    ..Default::default()
                },
            ),
        ]);
        let tree = label_tree(&me(), &labels, &prefs, &stats);
        assert_eq!(tree.pinned[0].name, "Receipts");
        let receipts = tree
            .own
            .iter()
            .find(|n| n.segment == "Authnet Auto-Receipt")
            .unwrap();
        assert_eq!(receipts.name, "Receipts");
        assert_eq!(receipts.color.as_deref(), Some("#2456b8"));
        let ai = &tree.other_apps[2].labels[0];
        assert!(
            ai.children
                .iter()
                .find(|n| n.name == "Social")
                .unwrap()
                .shown
        );
    }

    #[test]
    fn knows_other_apps_by_name() {
        assert_eq!(
            app_of("[Imap]/Sent"),
            Some(("Mail apps over IMAP".into(), "Sent".into()))
        );
        assert_eq!(
            app_of("Sync Issues/Conflicts"),
            Some(("Mail apps over IMAP".into(), "Sync Issues/Conflicts".into()))
        );
        assert_eq!(
            app_of("[Mailbox]/Later"),
            Some(("Mailbox".into(), "Later".into()))
        );
        assert_eq!(app_of("Work/Clients"), None);
        assert_eq!(app_of("Notes for Ada"), None);
    }
}
