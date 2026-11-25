//! Turning one message's change into what to do to its copies
//! (docs/imap.md, "Changing mail"). Folders are labels, so a message's
//! labels are the folders holding a copy of it:
//!
//! - read, unread, star and unstar set or clear `\Seen` and `\Flagged` on
//!   every copy;
//! - a label added is a copy made in its folder, and a label removed is
//!   that folder's copy deleted; a label removed and another added is a
//!   move;
//! - removing the last folder a message is in (archiving a message that is
//!   only in the inbox) moves it to the archive folder, or, from Trash or
//!   Junk, back to the inbox: mail is never deleted by a label change;
//! - adding Trash (or Junk) moves one copy there and deletes the others,
//!   as Gmail drops a trashed message's other labels.
//!
//! A folder that holds every message (Gmail's All Mail, `\All` on Gmail)
//! is never moved from or deleted from, except into Trash or Junk: there
//! mail leaves it only that way, and deleting its copy would delete the
//! message. Removing its label is left alone, and the label comes back by
//! sync. A `\All` folder elsewhere is a view of the other folders' mail
//! ([`super::super::folders::view_of_all`]): its copies are never changed
//! or counted as keeping a message, so archiving there moves the inbox
//! copy to a real archive folder; and mail is never filed in one (a label
//! kept from before the folder became a view): that change is refused.
//!
//! A copy asked for and not yet confirmed or ruled out (an
//! [`postrinch_model::ImapCopyAttempt`]) is not made again: its folder
//! counts as having the message for labels added, and for nothing else.

use std::collections::{BTreeSet, HashMap, HashSet};

use postrinch_model::{FolderRole, ImapFolder, ImapPlace, MessageChange, SystemLabel};

use super::super::folders::view_of_all;
use super::super::places::{DELETED, FLAGGED, SEEN, first_copy};
use super::commands::shown;
use crate::imap::folders::{ListedFolder, SpecialUse};

/// The account's folders as the outbox needs them.
#[derive(Debug, Clone, Default)]
pub(super) struct Folders {
    by_label: HashMap<String, ImapFolder>,
    /// Label ids of the folders that hold every message (Gmail's All Mail).
    everything: HashSet<String>,
    /// Label ids of `\All` folders elsewhere, kept from before they were
    /// left out of sync: views, never copies.
    views: HashSet<String>,
}

impl Folders {
    /// The folders kept, with which of them LIST marks `\All`, on Gmail
    /// (`gmail`) or elsewhere.
    pub(super) fn new(stored: Vec<ImapFolder>, listed: &[ListedFolder], gmail: bool) -> Self {
        let (mut all, mut views) = (HashSet::new(), HashSet::new());
        for f in listed
            .iter()
            .filter(|f| f.special_use == Some(SpecialUse::All))
        {
            match view_of_all(f, gmail) {
                true => views.insert(f.name.as_str()),
                false => all.insert(f.name.as_str()),
            };
        }
        let labels = |names: &HashSet<&str>| -> HashSet<String> {
            stored
                .iter()
                .filter(|f| names.contains(f.name.as_str()))
                .map(|f| f.label_id.clone())
                .collect()
        };
        let (everything, views) = (labels(&all), labels(&views));
        let by_label = stored
            .into_iter()
            .map(|f| (f.label_id.clone(), f))
            .collect();
        Self {
            by_label,
            everything,
            views,
        }
    }

    pub(super) fn get(&self, label: &str) -> Option<&ImapFolder> {
        self.by_label.get(label)
    }

    /// Keeps a folder made since the folders were read.
    pub(super) fn insert(&mut self, folder: ImapFolder) {
        self.by_label.insert(folder.label_id.clone(), folder);
    }

    /// Where archiving moves mail.
    pub(super) fn archive(&self) -> Option<&ImapFolder> {
        let mut found: Vec<&ImapFolder> = self
            .by_label
            .values()
            .filter(|f| f.role == Some(FolderRole::Archive) && !self.is_view(&f.label_id))
            .collect();
        found.sort_by(|a, b| a.label_id.cmp(&b.label_id));
        found.into_iter().next()
    }

    /// Whether the folder holds every message (Gmail's All Mail).
    pub(super) fn holds_everything(&self, label: &str) -> bool {
        self.everything.contains(label)
    }

    /// Whether the account has a folder that holds every message, so
    /// mail is kept whatever folders it leaves (Gmail).
    pub(super) fn keeps_everything(&self) -> bool {
        !self.everything.is_empty()
    }

    /// Whether the folder is a view of the other folders' mail (`\All`
    /// other than Gmail's), whose copies are not copies.
    pub(super) fn is_view(&self, label: &str) -> bool {
        self.views.contains(label)
    }

    /// Every folder, in no particular order.
    pub(super) fn all(&self) -> impl Iterator<Item = &ImapFolder> {
        self.by_label.values()
    }
}

/// One thing to do to one copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Op {
    /// `UID STORE ±FLAGS.SILENT (<flag>)`.
    Flag {
        place: ImapPlace,
        flag: &'static str,
        add: bool,
    },
    /// `UID COPY` into the folder that is label `to`.
    Copy { from: ImapPlace, to: String },
    /// `UID MOVE` (or its fallbacks) into the folder that is label `to`.
    Move { from: ImapPlace, to: String },
    /// `\Deleted` and an expunge.
    Delete { place: ImapPlace },
}

/// What one message's change comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Planned {
    /// These, in this order (flags, copies, moves, deletes); none when
    /// the server has no copy of the message left to change.
    Ops(Vec<Op>),
    /// The change cannot be made to this message, for this reason.
    Refused(String),
    /// Removing the message's last folder files it in the archive, and
    /// the account has no archive folder yet.
    NeedsArchive,
}

fn inbox() -> &'static str {
    SystemLabel::Inbox.provider_id()
}

fn is_bin(label: &str) -> bool {
    label == SystemLabel::Trash.provider_id() || label == SystemLabel::Spam.provider_id()
}

fn live(place: &ImapPlace) -> bool {
    !place.flags.iter().any(|f| f.eq_ignore_ascii_case(DELETED))
}

/// Whether a kept copy is one a change acts on and counts: not marked
/// deleted, in a folder still there, and not in a view of other folders.
pub(super) fn counted(place: &ImapPlace, folders: &Folders) -> bool {
    live(place) && folders.get(&place.folder).is_some() && !folders.is_view(&place.folder)
}

/// Each label once, in the order named.
fn each_once(labels: &[String]) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    labels
        .iter()
        .map(String::as_str)
        .filter(|l| seen.insert(*l))
        .collect()
}

/// What to do to a message's copies (`places`, as kept) for `change`;
/// `attempted`, the label ids of the folders a copy of it was asked for in
/// and not yet confirmed or ruled out.
pub(super) fn plan(
    places: &[ImapPlace],
    attempted: &[String],
    change: &MessageChange,
    folders: &Folders,
) -> Planned {
    let live: Vec<&ImapPlace> = places.iter().filter(|p| counted(p, folders)).collect();
    if live.is_empty() {
        return Planned::Ops(Vec::new());
    }
    let mut ops = Vec::new();
    for place in &live {
        for (wanted, flag, add_when) in [
            (change.unread, SEEN, false),
            (change.starred, FLAGGED, true),
        ] {
            if let Some(value) = wanted {
                ops.push(Op::Flag {
                    place: (*place).clone(),
                    flag,
                    add: value == add_when,
                });
            }
        }
    }
    let adds = each_once(&change.add_labels);
    // A label both added and removed ends up added, as `MessageChange::apply` has it.
    let removes: Vec<&str> = each_once(&change.remove_labels)
        .into_iter()
        .filter(|r| !adds.contains(r))
        .collect();
    if adds.iter().any(|a| folders.get(a).is_none()) {
        return Planned::Refused("a folder this change files mail in is gone".to_owned());
    }
    if let Some(view) = adds.iter().find(|a| folders.is_view(a)) {
        let name = folders.get(view).map_or("", |f| f.name.as_str());
        return Planned::Refused(format!(
            "{} is a view of the mail in the other folders, and mail cannot be filed in it",
            shown(name)
        ));
    }
    let bin = [SystemLabel::Trash, SystemLabel::Spam]
        .map(|l| l.provider_id())
        .into_iter()
        .find(|b| adds.contains(b));
    match bin {
        Some(bin) => into_bin(&live, bin, folders, &mut ops),
        None => match relabel(&live, attempted, &adds, &removes, folders, &mut ops) {
            Ok(()) => {}
            Err(other) => return other,
        },
    }
    Planned::Ops(ops)
}

/// Trash or Junk: one copy moved there (or the one there kept), every
/// other deleted.
fn into_bin(live: &[&ImapPlace], bin: &str, folders: &Folders, ops: &mut Vec<Op>) {
    let kept = match live.iter().find(|p| p.folder == bin) {
        Some(kept) => *kept,
        None => {
            let from = live
                .iter()
                .find(|p| !folders.holds_everything(&p.folder))
                .unwrap_or(&live[0]);
            ops.push(Op::Move {
                from: (*from).clone(),
                to: bin.to_owned(),
            });
            *from
        }
    };
    for place in live {
        if *place != kept && !folders.holds_everything(&place.folder) {
            ops.push(Op::Delete {
                place: (*place).clone(),
            });
        }
    }
}

/// Labels added and removed: moves where one goes and another comes,
/// copies and deletes for the rest, and the last folder's copy moved
/// where the message is kept when it is in no folder.
fn relabel(
    live: &[&ImapPlace],
    attempted: &[String],
    adds: &[&str],
    removes: &[&str],
    folders: &Folders,
    ops: &mut Vec<Op>,
) -> Result<(), Planned> {
    let have: BTreeSet<&str> = live.iter().map(|p| p.folder.as_str()).collect();
    let made = |a: &&str| have.contains(a) || attempted.iter().any(|u| u == a);
    let mut to_add: Vec<&str> = adds.iter().copied().filter(|a| !made(a)).collect();
    let to_remove: Vec<&str> = removes
        .iter()
        .copied()
        .filter(|r| have.contains(r) && !folders.holds_everything(r))
        .collect();
    let mut remaining: BTreeSet<&str> = have.iter().chain(&to_add).copied().collect();
    for r in &to_remove {
        remaining.remove(r);
    }
    let copies_in = |folder: &str| -> Vec<&ImapPlace> {
        live.iter()
            .copied()
            .filter(|p| p.folder == folder)
            .collect()
    };
    let mut moves: Vec<(&str, String)> = Vec::new();
    let mut deletes: Vec<&str> = Vec::new();
    let mut unpaired: Vec<&str> = Vec::new();
    for r in &to_remove {
        if to_add.is_empty() {
            unpaired.push(r);
        } else {
            moves.push((r, to_add.remove(0).to_owned()));
        }
    }
    if remaining.is_empty() {
        // Every copy is to go: one goes where a message in no folder is
        // kept, and the rest are deleted.
        let Some((i, home)) = unpaired.iter().enumerate().find_map(|(i, r)| {
            let home = home_for(r, folders);
            match home {
                Some(h) if to_remove.contains(&h.as_str()) => None,
                other => Some((i, other)),
            }
        }) else {
            return Err(Planned::Refused(
                "a message must stay in a folder, and this would take it out of every one"
                    .to_owned(),
            ));
        };
        let Some(home) = home else {
            return Err(Planned::NeedsArchive);
        };
        moves.push((unpaired.remove(i), home));
    }
    deletes.extend(unpaired);
    if let Some(source) = first_copy(live.iter().copied()) {
        for to in to_add {
            ops.push(Op::Copy {
                from: source.clone(),
                to: to.to_owned(),
            });
        }
    }
    for (from, to) in moves {
        let mut copies = copies_in(from).into_iter();
        if let Some(first) = copies.next() {
            ops.push(Op::Move {
                from: first.clone(),
                to,
            });
        }
        ops.extend(copies.map(|p| Op::Delete { place: p.clone() }));
    }
    for folder in deletes {
        ops.extend(
            copies_in(folder)
                .into_iter()
                .map(|p| Op::Delete { place: p.clone() }),
        );
    }
    Ok(())
}

/// The copies `ops` leave where they are: counted ones neither moved nor
/// deleted, the copies a message keeps when its others are deleted.
pub(super) fn kept<'p>(
    places: &'p [ImapPlace],
    ops: &[Op],
    folders: &Folders,
) -> Vec<&'p ImapPlace> {
    let goes = |p: &ImapPlace| {
        ops.iter().any(|op| match op {
            Op::Move { from, .. } => from == p,
            Op::Delete { place } => place == p,
            Op::Flag { .. } | Op::Copy { .. } => false,
        })
    };
    places
        .iter()
        .filter(|p| counted(p, folders) && !goes(p))
        .collect()
}

/// Where a message taken out of its last folder, `from`, is kept: the
/// inbox when it leaves Trash or Junk, else the archive folder (`None`
/// when the account has none yet).
pub(super) fn home_for(from: &str, folders: &Folders) -> Option<String> {
    if is_bin(from) {
        return Some(inbox().to_owned());
    }
    folders.archive().map(|a| a.label_id.clone())
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
