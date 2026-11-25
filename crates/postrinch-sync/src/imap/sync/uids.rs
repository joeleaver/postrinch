//! Sets of UIDs kept as ranges, as IMAP writes them (`1:4,7,9:*`), so a
//! set the server sends (a FETCH's set, a `VANISHED` of a folder emptied
//! of a million messages) costs a few bytes a range, not four a UID.

use std::ops::RangeInclusive;

/// A set of UIDs: sorted ranges that neither overlap nor touch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct UidSet(Vec<(u32, u32)>);

impl UidSet {
    /// Parses IMAP's syntax. `*` is taken as the largest UID there can be;
    /// what cannot be read is left out.
    pub(super) fn parse(set: &str) -> Self {
        let number = |n: &str| match n {
            "*" => Some(u32::MAX),
            n => n.parse().ok(),
        };
        Self::from_ranges(
            set.split(',')
                .filter_map(|part| match part.split_once(':') {
                    Some((a, b)) => {
                        let (a, b) = (number(a)?, number(b)?);
                        Some(a.min(b)..=a.max(b))
                    }
                    None => number(part).map(|n| n..=n),
                }),
        )
    }

    /// The UIDs of these ranges, in any order, overlapping or not.
    pub(super) fn from_ranges(ranges: impl IntoIterator<Item = RangeInclusive<u32>>) -> Self {
        let mut set = Self::default();
        set.extend(ranges);
        set
    }

    /// Adds these ranges.
    pub(super) fn extend(&mut self, ranges: impl IntoIterator<Item = RangeInclusive<u32>>) {
        self.0.extend(
            ranges
                .into_iter()
                .filter(|r| r.start() <= r.end())
                .map(|r| (*r.start(), *r.end())),
        );
        self.0.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.0.len());
        for &(a, b) in &self.0 {
            match merged.last_mut() {
                Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        self.0 = merged;
    }

    pub(super) fn contains(&self, uid: u32) -> bool {
        // The last range starting at or below `uid`.
        let i = self.0.partition_point(|&(a, _)| a <= uid);
        i > 0 && uid <= self.0[i - 1].1
    }

    /// How many UIDs of the set are below `uid`.
    pub(super) fn count_below(&self, uid: u32) -> u64 {
        self.0
            .iter()
            .take_while(|&&(a, _)| a < uid)
            .map(|&(a, b)| u64::from(b.min(uid - 1) - a) + 1)
            .sum()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The set's ranges, in order.
    pub(super) fn ranges(&self) -> impl Iterator<Item = RangeInclusive<u32>> + '_ {
        self.0.iter().map(|&(a, b)| a..=b)
    }
}

/// A UID set in IMAP's syntax, runs folded into ranges: `1:4,7,9:10`.
pub(super) fn uid_set(uids: &[u32]) -> String {
    let set = UidSet::from_ranges(uids.iter().map(|&u| u..=u));
    let mut out = String::new();
    for &(a, b) in &set.0 {
        if !out.is_empty() {
            out.push(',');
        }
        if a == b {
            out.push_str(&a.to_string());
        } else {
            out.push_str(&format!("{a}:{b}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_sets_fold_runs() {
        assert_eq!(uid_set(&[7, 1, 2, 3, 4, 9, 10, 2]), "1:4,7,9:10");
        assert_eq!(uid_set(&[5]), "5");
        assert_eq!(uid_set(&[]), "");
    }

    #[test]
    fn uid_sets_parse() {
        let set = UidSet::parse("1:4,7,20:*");
        assert!(set.contains(1) && set.contains(4) && set.contains(7));
        assert!(!set.contains(5) && !set.contains(19) && !set.contains(0));
        assert!(set.contains(20) && set.contains(u32::MAX));
        assert!(UidSet::parse("9:3").contains(5));
    }

    #[test]
    fn ranges_merge_and_count() {
        let set = UidSet::from_ranges([10..=20, 1..=3, 4..=4, 15..=25]);
        assert_eq!(set.0, [(1, 4), (10, 25)]);
        assert_eq!(set.count_below(1), 0);
        assert_eq!(set.count_below(3), 2);
        assert_eq!(set.count_below(12), 6);
        assert_eq!(set.count_below(100), 20);
        // A huge range is two numbers.
        let big = UidSet::from_ranges(std::iter::once(1..=u32::MAX - 1));
        assert_eq!(big.0.len(), 1);
        assert!(big.contains(4_000_000_000));
    }
}
