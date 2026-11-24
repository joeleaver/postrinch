//! Sequence sets (`1,3:5,7:*`), for message sequence numbers and UIDs.

/// One end of a range: a number, or `*` (the largest number in use).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    Num(u32),
    Star,
}

/// A parsed sequence set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeqSet {
    ranges: Vec<(Bound, Bound)>,
}

impl SeqSet {
    /// Parses `1,3:5,7:*`. Zero is not a valid number.
    pub(crate) fn parse(text: &str) -> Option<SeqSet> {
        let mut ranges = Vec::new();
        for item in text.split(',') {
            let (a, b) = match item.split_once(':') {
                Some((a, b)) => (bound(a)?, bound(b)?),
                None => {
                    let a = bound(item)?;
                    (a, a)
                }
            };
            ranges.push((a, b));
        }
        Some(SeqSet { ranges })
    }

    /// Whether `n` is in the set, where `*` stands for `largest`. A range
    /// is unordered (`5:2` is `2:5`), and `*` stands for the largest number
    /// even when that is below the other end, so `UID FETCH 100:*` on a
    /// mailbox whose largest UID is 40 matches UID 40 (RFC 9051 6.4.9).
    pub(crate) fn contains(&self, n: u32, largest: u32) -> bool {
        let value = |b: Bound| match b {
            Bound::Num(v) => v,
            Bound::Star => largest,
        };
        self.ranges.iter().any(|&(a, b)| {
            let (a, b) = (value(a), value(b));
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            (lo..=hi).contains(&n)
        })
    }
}

fn bound(text: &str) -> Option<Bound> {
    if text == "*" {
        return Some(Bound::Star);
    }
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match text.parse::<u32>().ok()? {
        0 => None,
        n => Some(Bound::Num(n)),
    }
}

/// Writes sorted numbers compactly: `1:3,5,9:10`.
pub(crate) fn format_set(numbers: &[u32]) -> String {
    let mut sorted = numbers.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            out.push_str(&start.to_string());
        } else {
            out.push_str(&format!("{start}:{end}"));
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_contains() {
        let set = SeqSet::parse("1,3:5,9:*").unwrap();
        assert!(set.contains(1, 12));
        assert!(!set.contains(2, 12));
        assert!(set.contains(4, 12));
        assert!(set.contains(12, 12));
        assert!(!set.contains(13, 12));
    }

    #[test]
    fn star_below_start() {
        let set = SeqSet::parse("100:*").unwrap();
        assert!(set.contains(40, 40));
        assert!(!set.contains(39, 40));
        let reversed = SeqSet::parse("5:2").unwrap();
        assert!(reversed.contains(3, 10));
    }

    #[test]
    fn invalid() {
        assert!(SeqSet::parse("").is_none());
        assert!(SeqSet::parse("0").is_none());
        assert!(SeqSet::parse("1:").is_none());
        assert!(SeqSet::parse("a").is_none());
        assert!(SeqSet::parse("1,,2").is_none());
    }

    #[test]
    fn formatting() {
        assert_eq!(format_set(&[5, 1, 2, 3, 9, 10]), "1:3,5,9:10");
        assert_eq!(format_set(&[]), "");
    }
}
