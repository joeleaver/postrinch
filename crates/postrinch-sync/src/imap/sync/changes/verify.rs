//! Asking the server whether the copies kept are still there, before a
//! change or a label's deletion counts on them (docs/imap.md, "Keeping
//! every message safe").
//!
//! The store's copies are what the last sync read: another client may
//! have deleted one since, or the folder's UIDVALIDITY may have changed,
//! so its UIDs name other messages now. So every copy of a message whose
//! change copies, moves or deletes anything is looked for on the server
//! first: in its folder, at the folder's UIDVALIDITY now, not marked
//! `\Deleted`. Copies found gone are forgotten and the message's change
//! planned again from the rest, so a copy or a move is made from a copy
//! that is there, never from one that is gone (a command on a UID that is
//! gone does nothing). The copies found that the change leaves in place
//! are what lets its deletes go ([`super::apply`]); a message whose
//! change only deletes and that keeps none has a copy moved where a
//! message in no folder goes instead, so it is never lost. The checks are
//! one `UID SEARCH` per folder for every message of the change at once.

use std::collections::{BTreeMap, HashMap, HashSet};

use postrinch_model::{EmailAddress, ImapPlace, MessageChange, ProviderMessageId};

use super::super::ImapSync;
use super::apply::{Exec, uids};
use super::change::{Held, Verdict};
use super::commands::CHUNK;
use super::failure::{Failure, Outcome};
use super::plan::{Folders, Op, counted, home_for, kept};
use super::record;

/// A copy as the server names it: its folder, UIDVALIDITY and UID.
pub(super) type Where = (String, u32, u32);

pub(super) fn where_of(place: &ImapPlace) -> Where {
    (place.folder.clone(), place.uidvalidity, place.uid)
}

/// What the server said of the copies looked for.
#[derive(Default)]
pub(super) struct Checked {
    /// In their folder at its UIDVALIDITY now, not marked `\Deleted`.
    pub there: HashSet<Where>,
    /// Not there, at the folder's UIDVALIDITY now: forgotten.
    pub gone: HashSet<Where>,
}

/// Why a message's deletes wait when the search could not say whether
/// its other copies are there.
pub(super) const UNANSWERED: &str = "the server would not say whether a message's other copy \
     is still there, so the copy the change deletes stays; the change is tried again";

fn deletes(ops: &[Op]) -> bool {
    ops.iter().any(|op| matches!(op, Op::Delete { .. }))
}

fn transfers(ops: &[Op]) -> bool {
    ops.iter()
        .any(|op| matches!(op, Op::Copy { .. } | Op::Move { .. }))
}

impl ImapSync {
    /// Makes sure each message whose ops copy, move or delete do so from
    /// what the server has, as the module says, and returns the messages
    /// that keep a copy the server was seen to have, which the change
    /// leaves in place, with that copy. A message whose change only
    /// deletes and whose other copies the search found all gone has a copy
    /// moved where a message in no folder goes instead (the archive, or
    /// from Trash or Junk the inbox), or, where that is the folder it is
    /// in, keeps its copies and is refused. Where the server would not say
    /// (a folder it would not open or search, or whose UIDVALIDITY
    /// changed), its deletes wait.
    pub(super) async fn keep_one_copy(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &mut Folders,
        (held, change): (&[Held], &MessageChange),
        planned: &mut [Vec<Op>],
        verdict: &mut Verdict,
    ) -> Outcome<HashMap<ProviderMessageId, ImapPlace>> {
        let at_risk: Vec<usize> = (0..planned.len())
            .filter(|&i| deletes(&planned[i]) || transfers(&planned[i]))
            .collect();
        let mut anchored = HashMap::new();
        if at_risk.is_empty() {
            return Ok(anchored);
        }
        let all: Vec<ImapPlace> = at_risk
            .iter()
            .flat_map(|&i| held[i].places.iter().filter(|p| counted(p, folders)))
            .cloned()
            .collect();
        let checked = self.still_there(exec, account, folders, all).await?;
        for i in at_risk {
            let left = Held {
                places: held[i]
                    .places
                    .iter()
                    .filter(|p| !checked.gone.contains(&where_of(p)))
                    .cloned()
                    .collect(),
                attempted: held[i].attempted.clone(),
            };
            if left.places.len() < held[i].places.len() {
                planned[i] = self
                    .plan_one(exec.conn, account, folders, &left, change, verdict)
                    .await?;
            }
            let kept = kept(&left.places, &planned[i], folders);
            if let Some(found) = kept.iter().find(|p| checked.there.contains(&where_of(p))) {
                anchored.insert(found.message.clone(), (*found).clone());
            } else if deletes(&planned[i]) && !transfers(&planned[i]) {
                if kept.is_empty() {
                    // The search found every other copy gone.
                    self.file_instead(exec, account, folders, &mut planned[i], verdict)
                        .await?;
                } else {
                    // A refused search is no evidence either way: the
                    // copies stay as they are, and the change waits.
                    planned[i].retain(|op| !matches!(op, Op::Delete { .. }));
                    verdict.waits.get_or_insert(UNANSWERED.to_owned());
                }
            }
        }
        Ok(anchored)
    }

    /// One message's first delete made a move where a message in no folder
    /// goes.
    async fn file_instead(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &mut Folders,
        ops: &mut Vec<Op>,
        verdict: &mut Verdict,
    ) -> Outcome<()> {
        let Some(i) = ops.iter().position(|op| matches!(op, Op::Delete { .. })) else {
            return Ok(());
        };
        let Op::Delete { place } = ops[i].clone() else {
            return Ok(());
        };
        if home_for(&place.folder, folders).is_none() {
            self.make_archive(exec.conn, account, folders).await?;
        }
        match home_for(&place.folder, folders) {
            Some(home) if home != place.folder => {
                ops[i] = Op::Move {
                    from: place,
                    to: home,
                };
            }
            _ => {
                ops.retain(|op| !matches!(op, Op::Delete { .. }));
                verdict.refused.get_or_insert(
                    "a message must stay in a folder, and its other copies are gone from \
                     the server"
                        .to_owned(),
                );
            }
        }
        Ok(())
    }

    /// Which of these kept copies the server still has, as [`Where`]s:
    /// in their folder at its UIDVALIDITY now, not marked `\Deleted`. A
    /// folder that cannot be opened, or will not be searched, counts as
    /// having none, and none gone. Copies found gone at the folder's
    /// UIDVALIDITY are forgotten; those of an older one are neither there
    /// nor gone, left for sync to read again.
    pub(super) async fn still_there(
        &self,
        exec: &mut Exec<'_>,
        account: &EmailAddress,
        folders: &Folders,
        places: Vec<ImapPlace>,
    ) -> Outcome<Checked> {
        let mut by_folder: BTreeMap<String, Vec<ImapPlace>> = BTreeMap::new();
        for place in places {
            by_folder
                .entry(place.folder.clone())
                .or_default()
                .push(place);
        }
        let (mut checked, mut gone) = (Checked::default(), Vec::new());
        for (label, places) in by_folder {
            let validity = match exec.open(folders, &label).await {
                Ok(validity) => validity,
                Err(Failure::Refused(_) | Failure::Declined(_)) => continue,
                Err(other) => return Err(other),
            };
            let now: Vec<ImapPlace> = places
                .into_iter()
                .filter(|p| p.uidvalidity == validity)
                .collect();
            for chunk in now.chunks(CHUNK) {
                let Some(present) = exec.conn.present(&uids(chunk)).await? else {
                    continue;
                };
                for place in chunk {
                    if present.contains(&place.uid) {
                        checked.there.insert(where_of(place));
                    } else {
                        checked.gone.insert(where_of(place));
                        gone.push(place.clone());
                    }
                }
            }
        }
        if !gone.is_empty() {
            let a = account.clone();
            self.blocking(move |s| gone.iter().try_for_each(|p| record::remove_copy(s, &a, p)))
                .await?;
        }
        Ok(checked)
    }
}
