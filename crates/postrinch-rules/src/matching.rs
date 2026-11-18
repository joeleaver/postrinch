//! Whether a message matches a rule, decided here: for rules Gmail cannot
//! run, for "also apply to existing mail", and for rules that run as mail
//! ages.

use postrinch_model::{Mailbox, Message, Timestamp};

use crate::words::{has_all, has_phrase, words};
use crate::{Condition, Match, Rule, Test};

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;
const INBOX: &str = "INBOX";

/// A rule's answer for one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Yes,
    No,
    /// It depends on the body or attachments, which aren't downloaded yet.
    NotYet,
}

impl Verdict {
    fn from(yes: bool) -> Self {
        if yes { Verdict::Yes } else { Verdict::No }
    }

    fn flip(self) -> Self {
        match self {
            Verdict::Yes => Verdict::No,
            Verdict::No => Verdict::Yes,
            Verdict::NotYet => Verdict::NotYet,
        }
    }
}

/// Whether `message` matches `rule`'s conditions, at `now`. Whether the
/// rule is on, and when it runs, are the caller's to check.
pub fn matches(rule: &Rule, message: &Message, now: Timestamp) -> Verdict {
    let mut verdicts = rule.conditions.iter().map(|c| condition(c, message, now));
    match rule.matching {
        Match::All => {
            let mut waiting = false;
            for v in verdicts {
                match v {
                    Verdict::No => return Verdict::No,
                    Verdict::NotYet => waiting = true,
                    Verdict::Yes => {}
                }
            }
            if waiting {
                Verdict::NotYet
            } else {
                Verdict::Yes
            }
        }
        Match::Any => {
            let mut waiting = false;
            if verdicts.any(|v| {
                waiting |= v == Verdict::NotYet;
                v == Verdict::Yes
            }) {
                return Verdict::Yes;
            }
            if waiting {
                Verdict::NotYet
            } else {
                Verdict::No
            }
        }
    }
}

fn condition(condition: &Condition, message: &Message, now: Timestamp) -> Verdict {
    let verdict = if condition.test.needs_content() && !message.downloaded {
        Verdict::NotYet
    } else {
        Verdict::from(test(&condition.test, message, now))
    };
    if condition.negate {
        verdict.flip()
    } else {
        verdict
    }
}

fn key(address: &str) -> String {
    address.trim().to_lowercase()
}

fn any_is(mailboxes: &[Mailbox], address: &str) -> bool {
    let want = key(address);
    mailboxes.iter().any(|m| m.address.key() == want)
}

fn mailbox_words(mailboxes: &[Mailbox]) -> Vec<String> {
    let mut all = Vec::new();
    for m in mailboxes {
        if let Some(name) = &m.name {
            all.extend(words(name));
        }
        all.extend(words(&m.address.to_string()));
    }
    all
}

/// Everything Gmail searches for a message's words: subject, body,
/// senders and recipients, and attachment names.
fn all_words(message: &Message) -> Vec<String> {
    let h = &message.headers;
    let mut all = words(message.subject());
    all.extend(words(&message.body_text));
    for list in [&h.from, &h.to, &h.cc, &h.bcc] {
        all.extend(mailbox_words(list));
    }
    for a in &message.attachments {
        if let Some(name) = &a.filename {
            all.extend(words(name));
        }
    }
    all
}

fn test(test: &Test, message: &Message, now: Timestamp) -> bool {
    let h = &message.headers;
    match test {
        Test::FromAddress(a) => any_is(&h.from, a),
        Test::FromDomain(d) => {
            let want = d.trim().trim_start_matches('@').to_lowercase();
            h.from.iter().any(|m| m.address.domain() == want)
        }
        Test::FromWords(w) => has_all(&mailbox_words(&h.from), &words(w)),
        Test::RecipientAddress(a) => any_is(&h.to, a) || any_is(&h.cc, a) || any_is(&h.bcc, a),
        Test::ToAddress(a) => any_is(&h.to, a),
        Test::CcAddress(a) => any_is(&h.cc, a),
        Test::SubjectWords(w) => has_all(&words(message.subject()), &words(w)),
        Test::SubjectPhrase(p) => has_phrase(&words(message.subject()), &words(p)),
        Test::Words(w) => has_all(&all_words(message), &words(w)),
        Test::Phrase(p) => {
            has_phrase(&words(message.subject()), &words(p))
                || has_phrase(&words(&message.body_text), &words(p))
        }
        Test::HasAttachment => message.has_attachments(),
        Test::AttachmentType(t) => {
            let ext = format!(".{}", t.trim().trim_start_matches('.').to_lowercase());
            message
                .attachments
                .iter()
                .filter_map(|a| a.filename.as_deref())
                .any(|n| n.to_lowercase().ends_with(&ext))
        }
        Test::AttachmentName(n) => {
            let want = n.trim().to_lowercase();
            message
                .attachments
                .iter()
                .filter_map(|a| a.filename.as_deref())
                .any(|f| f.to_lowercase() == want)
        }
        Test::LargerThan(n) => message.size > *n,
        Test::SmallerThan(n) => message.size < *n,
        Test::Unread => message.flags.unread,
        Test::Starred => message.flags.starred,
        Test::HasLabel(l) => message.has_label(l),
        Test::InInbox => message.has_label(INBOX),
        Test::OlderThanDays(d) => {
            now.as_millis() - message.received.as_millis() > i64::from(*d) * DAY_MILLIS
        }
    }
}

#[cfg(test)]
mod tests;
