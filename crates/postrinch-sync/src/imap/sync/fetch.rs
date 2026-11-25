//! Reading a `UID FETCH`'s responses into one answer per message.
//!
//! A server may answer one message in several FETCH responses (RFC 3501
//! 7.4.2: the items of one message need not come together), and may send
//! FETCH responses nobody asked for while a command runs: under CONDSTORE
//! and QRESYNC, `* n FETCH (UID x FLAGS (...) MODSEQ (m))` when another
//! client changes flags. So the responses are merged by message, a
//! response without a UID joins the message its sequence number names,
//! and only the messages in the set asked for are answers.
//!
//! Unsolicited flag updates for messages outside that set are dropped.
//! That loses nothing: a folder's HIGHESTMODSEQ is kept as SELECT gave
//! it, before any such update, so the next pass's `CHANGEDSINCE` reads
//! the change again (and without CONDSTORE every pass reads every flag).

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use async_imap::imap_proto::{AttributeValue, MessageSection, SectionPath};

use super::uids::UidSet;
use super::wire::Refusal;

/// One message's attributes from the FETCH responses of a command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Fetched {
    pub uid: u32,
    pub flags: Option<Vec<String>>,
    pub modseq: Option<u64>,
    pub size: Option<u32>,
    pub internal_date: Option<String>,
    /// `BODY[HEADER]`; empty when the server sent `NIL`.
    pub header: Option<Vec<u8>>,
    /// `BODY[]`; empty when the server sent `NIL`.
    pub body: Option<Vec<u8>>,
}

impl Fetched {
    /// Takes what `newer` carries. Flags go with their MODSEQ: older
    /// flags (a lower MODSEQ) do not replace newer ones.
    fn merge(&mut self, newer: Fetched) {
        if let Some(flags) = newer.flags {
            let older =
                matches!((self.modseq, newer.modseq), (Some(have), Some(new)) if new < have);
            if !older {
                self.flags = Some(flags);
                self.modseq = newer.modseq.or(self.modseq);
            }
        } else if newer
            .modseq
            .is_some_and(|new| self.modseq.is_none_or(|have| new > have))
        {
            self.modseq = newer.modseq;
        }
        self.size = newer.size.or(self.size);
        self.internal_date = newer.internal_date.or(self.internal_date.take());
        self.header = newer.header.or(self.header.take());
        self.body = newer.body.or(self.body.take());
    }
}

/// What a `UID FETCH` brought: the messages asked for, in UID order, the
/// UIDs the server said had vanished (QRESYNC), and, when the command
/// ended in `NO`, why: the server answered for some messages and not the
/// others (Gmail's "Some messages could not be FETCHed", Dovecot's
/// `[SERVERBUG]`), and the answers that came are still good.
#[derive(Debug, Default)]
pub(super) struct FetchReply {
    pub messages: Vec<Fetched>,
    pub vanished: UidSet,
    pub refused: Option<Refusal>,
}

/// Gathers a command's FETCH responses.
pub(super) struct Collector {
    asked: UidSet,
    by_uid: BTreeMap<u32, Fetched>,
    /// The UID each sequence number named in this command.
    by_seq: BTreeMap<u32, u32>,
    vanished: UidSet,
}

impl Collector {
    /// For a command asking for the UIDs in `set` (IMAP's syntax).
    pub(super) fn new(set: &str) -> Self {
        Self {
            asked: UidSet::parse(set),
            by_uid: BTreeMap::new(),
            by_seq: BTreeMap::new(),
            vanished: UidSet::default(),
        }
    }

    /// One `* seq FETCH (...)` response.
    pub(super) fn fetch(&mut self, seq: u32, attributes: &[AttributeValue<'_>]) {
        let (uid, data) = attributes_of(attributes);
        let Some(uid) = uid.or_else(|| self.by_seq.get(&seq).copied()) else {
            // Neither a UID nor a message seen in this command: an
            // unsolicited update for a message not asked for.
            return;
        };
        self.by_seq.insert(seq, uid);
        self.by_uid
            .entry(uid)
            .or_insert_with(|| Fetched {
                uid,
                ..Fetched::default()
            })
            .merge(Fetched { uid, ..data });
    }

    /// `* seq EXPUNGE`: the sequence numbers above it move down by one.
    pub(super) fn expunge(&mut self, seq: u32) {
        let above = self.by_seq.split_off(&seq);
        for (s, uid) in above {
            if s > seq {
                self.by_seq.insert(s - 1, uid);
            }
        }
    }

    /// `* VANISHED [(EARLIER)] <set>`. Without `EARLIER` the messages went
    /// just now, as by `EXPUNGE` (RFC 7162 3.2.10): the sequence numbers
    /// of the messages after each one move down by one. Sequence numbers
    /// go up with UIDs, so a message's new number is its old one less
    /// the vanished UIDs below its own.
    pub(super) fn vanished(
        &mut self,
        earlier: bool,
        ranges: impl IntoIterator<Item = RangeInclusive<u32>>,
    ) {
        let gone = UidSet::from_ranges(ranges);
        if !earlier && !gone.is_empty() {
            self.by_seq = std::mem::take(&mut self.by_seq)
                .into_iter()
                .filter(|&(_, uid)| !gone.contains(uid))
                .filter_map(|(seq, uid)| {
                    let below = u32::try_from(gone.count_below(uid)).ok()?;
                    Some((seq.checked_sub(below)?, uid))
                })
                .collect();
        }
        self.vanished.extend(gone.ranges());
    }

    /// The answers: the messages asked for.
    pub(super) fn finish(self) -> FetchReply {
        let asked = self.asked;
        FetchReply {
            messages: self
                .by_uid
                .into_values()
                .filter(|f| asked.contains(f.uid))
                .collect(),
            vanished: self.vanished,
            refused: None,
        }
    }
}

/// A FETCH response's UID, if it has one, and its other attributes.
fn attributes_of(attributes: &[AttributeValue<'_>]) -> (Option<u32>, Fetched) {
    let mut out = Fetched::default();
    let mut uid = None;
    // NIL (a server with nothing to give) is kept as empty, so the
    // message is still answered.
    let bytes = |data: &Option<std::borrow::Cow<'_, [u8]>>| {
        Some(data.as_ref().map_or_else(Vec::new, |d| d.to_vec()))
    };
    for attribute in attributes {
        match attribute {
            AttributeValue::Uid(u) => uid = Some(*u),
            AttributeValue::Flags(flags) => {
                out.flags = Some(flags.iter().map(|f| f.to_string()).collect());
            }
            AttributeValue::ModSeq(m) => out.modseq = Some(*m),
            AttributeValue::Rfc822Size(s) => out.size = Some(*s),
            AttributeValue::InternalDate(d) => out.internal_date = Some(d.to_string()),
            AttributeValue::BodySection { section, data, .. } => match section {
                Some(SectionPath::Full(MessageSection::Header)) => out.header = bytes(data),
                None => out.body = bytes(data),
                _ => {}
            },
            AttributeValue::Rfc822Header(data) => out.header = bytes(data),
            AttributeValue::Rfc822(data) => out.body = bytes(data),
            _ => {}
        }
    }
    (uid, out)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    fn flags(
        uid: Option<u32>,
        flags: &[&'static str],
        modseq: u64,
    ) -> Vec<AttributeValue<'static>> {
        let mut out = Vec::new();
        if let Some(uid) = uid {
            out.push(AttributeValue::Uid(uid));
        }
        out.push(AttributeValue::Flags(
            flags.iter().map(|f| Cow::Borrowed(*f)).collect(),
        ));
        out.push(AttributeValue::ModSeq(modseq));
        out
    }

    fn header(uid: Option<u32>, data: Option<&'static [u8]>) -> Vec<AttributeValue<'static>> {
        let mut out = Vec::new();
        if let Some(uid) = uid {
            out.push(AttributeValue::Uid(uid));
        }
        out.push(AttributeValue::BodySection {
            section: Some(SectionPath::Full(MessageSection::Header)),
            index: None,
            data: data.map(Cow::Borrowed),
        });
        out
    }

    #[test]
    fn split_responses_are_one_message() {
        let mut c = Collector::new("10");
        c.fetch(1, &flags(Some(10), &["\\Seen"], 5));
        c.fetch(1, &header(None, Some(b"Subject: x\r\n\r\n")));
        let reply = c.finish();
        assert_eq!(reply.messages.len(), 1);
        let m = &reply.messages[0];
        assert_eq!(m.uid, 10);
        assert_eq!(m.flags.as_deref(), Some(&["\\Seen".to_owned()][..]));
        assert_eq!(m.header.as_deref(), Some(&b"Subject: x\r\n\r\n"[..]));
    }

    #[test]
    fn unsolicited_updates_are_not_answers() {
        let mut c = Collector::new("10:11");
        // Another message's flags changed: not asked for.
        c.fetch(4, &flags(Some(3), &["\\Flagged"], 9));
        // One asked for, its flags first, then its answer.
        c.fetch(5, &flags(Some(10), &["\\Seen"], 8));
        c.fetch(5, &header(Some(10), Some(b"a")));
        let reply = c.finish();
        let uids: Vec<u32> = reply.messages.iter().map(|m| m.uid).collect();
        assert_eq!(uids, [10]);
        assert_eq!(reply.messages[0].modseq, Some(8));
    }

    #[test]
    fn older_flags_do_not_replace_newer_ones() {
        let mut c = Collector::new("1:*");
        c.fetch(1, &flags(Some(10), &["\\Seen", "\\Flagged"], 9));
        c.fetch(1, &flags(Some(10), &["\\Seen"], 7));
        let m = &c.finish().messages[0];
        assert_eq!(m.flags.as_ref().unwrap().len(), 2);
        assert_eq!(m.modseq, Some(9));
    }

    #[test]
    fn a_nil_header_is_empty_and_expunges_move_sequence_numbers() {
        let mut c = Collector::new("1:*");
        c.fetch(2, &flags(Some(20), &[], 1));
        c.fetch(3, &flags(Some(30), &[], 1));
        c.expunge(2);
        // Sequence number 2 is now UID 30.
        c.fetch(2, &header(None, None));
        let reply = c.finish();
        let thirty = reply.messages.iter().find(|m| m.uid == 30).unwrap();
        assert_eq!(thirty.header.as_deref(), Some(&[][..]));
        let twenty = reply.messages.iter().find(|m| m.uid == 20).unwrap();
        assert_eq!(twenty.header, None);
    }

    #[test]
    fn vanished_now_moves_sequence_numbers_and_earlier_does_not() {
        let mut c = Collector::new("1:*");
        c.fetch(1, &flags(Some(10), &[], 1));
        c.fetch(2, &flags(Some(20), &[], 1));
        c.fetch(3, &flags(Some(30), &[], 1));
        c.fetch(4, &flags(Some(40), &[], 1));
        // Earlier vanishings are not of this session's numbers.
        c.vanished(true, std::iter::once(5..=5));
        // 10 and 20 go now: 30 is number 1, 40 number 2.
        c.vanished(false, [10..=10, 20..=20]);
        c.fetch(1, &header(None, Some(b"thirty")));
        c.fetch(2, &header(None, Some(b"forty")));
        let reply = c.finish();
        let header_of = |uid| {
            let m = reply.messages.iter().find(|m| m.uid == uid).unwrap();
            m.header.clone()
        };
        assert_eq!(header_of(30).as_deref(), Some(&b"thirty"[..]));
        assert_eq!(header_of(40).as_deref(), Some(&b"forty"[..]));
        assert_eq!(header_of(10), None);
        assert!(reply.vanished.contains(5) && reply.vanished.contains(20));
    }
}
