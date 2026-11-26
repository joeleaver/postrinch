//! The order a server search's page walks what was found in
//! ([`super::search_page`]): newest first across the folders searched,
//! taking UIDs from each folder's top a batch at a time.
//!
//! Within a folder, UIDs rise as mail arrives, so a folder's highest UIDs
//! not yet walked are its newest; within each batch taken, when every
//! copy's arrival is known, by that (a message moved in keeps its date
//! but gets a new UID). Across folders, the walk takes the newest of each
//! folder's next (by when it arrived, `INTERNALDATE`, or the store's date
//! of a copy it has): a merge, which needs only each folder's next few in
//! hand, never all it found. Every folder with UIDs left has some in hand
//! before the newest is chosen, however many folders there are; how far
//! the walk goes is the page's to bound, by what it walks
//! ([`Walk::next`]), not by what is taken to be in hand.

use std::collections::{HashSet, VecDeque};

use postrinch_model::{ProviderMessageId, Timestamp};

use super::search::Hits;

/// One UID found, as it stands here.
#[derive(Debug, Clone)]
pub(super) struct Candidate {
    /// The folder's index among the hits.
    pub folder: usize,
    pub uid: u32,
    /// The message, when the store has it: this copy, or (named by its
    /// `Message-ID`) another. Nothing to fetch then.
    pub here: Option<ProviderMessageId>,
    /// The message it is, when that is known: the store's, or the one its
    /// `Message-ID` names. `None` for a copy without one, which is a
    /// message of its own.
    pub message: Option<ProviderMessageId>,
    pub arrived: Option<Timestamp>,
}

/// Where a walk of the found stands.
#[derive(Debug)]
pub(super) struct Walk {
    /// Per folder, the UIDs not taken yet, ascending: taken from the end.
    left: Vec<Vec<u32>>,
    /// Per folder, the candidates taken and not walked past, newest first.
    ready: Vec<VecDeque<Candidate>>,
}

impl Walk {
    /// A walk of `hits`.
    pub fn new(hits: &[Hits]) -> Self {
        Self {
            left: hits.iter().map(|h| h.uids.clone()).collect(),
            ready: hits.iter().map(|_| VecDeque::new()).collect(),
        }
    }

    /// The UIDs to take next, newest first, up to `step` (at least one)
    /// from each folder with none ready and some left: what must be in
    /// hand before [`Walk::next`] can say which is newest.
    pub fn take_next(&mut self, step: usize) -> Vec<(usize, Vec<u32>)> {
        let mut out = Vec::new();
        for (i, left) in self.left.iter_mut().enumerate() {
            if !self.ready[i].is_empty() || left.is_empty() {
                continue;
            }
            let n = step.max(1).min(left.len());
            let mut taken = left.split_off(left.len() - n);
            taken.reverse();
            out.push((i, taken));
        }
        out
    }

    /// Adds what was taken from a folder: newest first by arrival when
    /// each one's is known, otherwise in the order taken.
    pub fn add(&mut self, folder: usize, mut candidates: Vec<Candidate>) {
        if candidates.iter().all(|c| c.arrived.is_some()) {
            candidates.sort_by(|a, b| b.arrived.cmp(&a.arrived).then(b.uid.cmp(&a.uid)));
        }
        self.ready[folder].extend(candidates);
    }

    /// The newest candidate in hand, each folder's in the order added;
    /// `None` when none is. A copy whose arrival is not known comes after
    /// those whose is.
    pub fn next(&mut self) -> Option<Candidate> {
        let (folder, _) = self
            .ready
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.front().map(|c| (i, c)))
            // The first folder (the inbox ranks first) wins a tie.
            .rev()
            .max_by(|(_, a), (_, b)| a.arrived.cmp(&b.arrived).then(a.uid.cmp(&b.uid)))?;
        self.ready[folder].pop_front()
    }

    /// Whether some UID found is not walked yet and may be a message not
    /// `checked`: one not taken yet (which message it is is not known), or
    /// one in hand that is not a copy of a message checked.
    pub fn has_more(&self, checked: &HashSet<ProviderMessageId>) -> bool {
        self.left.iter().any(|l| !l.is_empty())
            || self.ready.iter().flatten().any(|c| {
                c.message
                    .as_ref()
                    .is_none_or(|message| !checked.contains(message))
            })
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::ImapFolder;

    use super::*;

    fn hits(uids: &[&[u32]]) -> Vec<Hits> {
        uids.iter()
            .enumerate()
            .map(|(i, u)| Hits {
                folder: ImapFolder::new(format!("f{i}"), format!("f{i}")),
                uidvalidity: 1,
                uids: u.to_vec(),
            })
            .collect()
    }

    fn at(folder: usize, uid: u32, when: i64) -> Candidate {
        Candidate {
            folder,
            uid,
            here: None,
            message: None,
            arrived: Some(Timestamp::from_millis(when)),
        }
    }

    /// Walks to the end, dating each UID by `when`.
    fn walk_all(
        walk: &mut Walk,
        step: usize,
        when: impl Fn(usize, u32) -> i64,
    ) -> Vec<(usize, u32)> {
        let mut out = Vec::new();
        loop {
            for (i, uids) in walk.take_next(step) {
                walk.add(i, uids.into_iter().map(|u| at(i, u, when(i, u))).collect());
            }
            let Some(c) = walk.next() else {
                return out;
            };
            out.push((c.folder, c.uid));
        }
    }

    #[test]
    fn the_walk_is_newest_first_across_folders() {
        let found = hits(&[&[1, 2, 3], &[7, 8]]);
        let mut walk = Walk::new(&found);
        // Folder 0's UID n arrived at day n·10, folder 1's at n·3.
        let order = walk_all(&mut walk, 1, |i, u| {
            i64::from(u) * if i == 0 { 10 } else { 3 }
        });
        assert_eq!(order, [(0, 3), (1, 8), (1, 7), (0, 2), (0, 1)]);
        assert!(!walk.has_more(&HashSet::new()));
    }

    #[test]
    fn a_folder_is_taken_from_only_when_its_ready_run_out() {
        let found = hits(&[&[1, 2, 3, 4, 5], &[9]]);
        let mut walk = Walk::new(&found);
        assert_eq!(walk.take_next(2), [(0, vec![5, 4]), (1, vec![9])]);
        walk.add(0, vec![at(0, 5, 50), at(0, 4, 40)]);
        walk.add(1, vec![at(1, 9, 10)]);
        assert_eq!(walk.take_next(2), []);
        assert_eq!(walk.next().map(|c| c.uid), Some(5));
        assert_eq!(walk.next().map(|c| c.uid), Some(4));
        assert_eq!(walk.take_next(2), [(0, vec![3, 2])]);
    }

    #[test]
    fn every_folder_has_some_in_hand_however_many_there_are() {
        // More folders than any bound a page would set on what it walks:
        // the newest, in the last folder, is still walked first.
        let mut uids: Vec<Vec<u32>> = (0..30).map(|_| (1..=60).collect()).collect();
        uids[29] = vec![1, 2];
        let slices: Vec<&[u32]> = uids.iter().map(Vec::as_slice).collect();
        let found = hits(&slices);
        let mut walk = Walk::new(&found);
        let taken = walk.take_next(50);
        assert_eq!(taken.len(), 30, "one take from every folder");
        assert_eq!(taken[29], (29, vec![2, 1]));
        for (i, uids) in taken {
            let when = |u: u32| {
                if i == 29 {
                    1000 + i64::from(u)
                } else {
                    i64::from(u)
                }
            };
            walk.add(i, uids.into_iter().map(|u| at(i, u, when(u))).collect());
        }
        assert_eq!(walk.next().map(|c| (c.folder, c.uid)), Some((29, 2)));
        assert_eq!(walk.next().map(|c| (c.folder, c.uid)), Some((29, 1)));
        assert!(walk.has_more(&HashSet::new()));
    }

    #[test]
    fn copies_in_hand_of_messages_checked_are_not_more() {
        let found = hits(&[&[1], &[1]]);
        let mut walk = Walk::new(&found);
        walk.take_next(1);
        let m = ProviderMessageId("m".into());
        let copy = |folder| Candidate {
            message: Some(m.clone()),
            ..at(folder, 1, 5)
        };
        walk.add(0, vec![copy(0)]);
        walk.add(1, vec![copy(1)]);
        walk.next();
        let checked = HashSet::from([m.clone()]);
        // Folder 1's copy is the message checked: nothing more to find.
        assert!(!walk.has_more(&checked));
        // Unchecked, it may still be found.
        assert!(walk.has_more(&HashSet::new()));
        // A copy whose message is unknown may be a message of its own.
        let mut walk = Walk::new(&found);
        walk.take_next(1);
        walk.add(0, vec![at(0, 1, 5)]);
        walk.add(1, vec![copy(1)]);
        assert!(walk.has_more(&checked));
    }

    #[test]
    fn a_batch_is_ordered_by_arrival_when_each_ones_is_known() {
        let found = hits(&[&[1, 2]]);
        let mut walk = Walk::new(&found);
        walk.take_next(2);
        // UID 1 arrived later (a message moved in keeps its date).
        walk.add(0, vec![at(0, 2, 5), at(0, 1, 9)]);
        assert_eq!(walk.next().map(|c| c.uid), Some(1));
        let mut walk = Walk::new(&found);
        walk.take_next(2);
        let mut undated = at(0, 2, 0);
        undated.arrived = None;
        walk.add(0, vec![undated, at(0, 1, 9)]);
        assert_eq!(walk.next().map(|c| c.uid), Some(2), "UIDs say, then");
    }

    #[test]
    fn copies_of_unknown_arrival_come_last() {
        let found = hits(&[&[1], &[2]]);
        let mut walk = Walk::new(&found);
        walk.take_next(1);
        let mut undated = at(1, 2, 0);
        undated.arrived = None;
        walk.add(0, vec![at(0, 1, 5)]);
        walk.add(1, vec![undated]);
        assert_eq!(walk.next().map(|c| c.folder), Some(0));
        assert_eq!(walk.next().map(|c| c.folder), Some(1));
    }
}
