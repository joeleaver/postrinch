use serde::{Deserialize, Serialize};

use crate::{EmailAddress, Mailbox, Message, SystemLabel, Timestamp};

/// The provider's identifier for a thread within one account: Gmail's
/// thread id, or for IMAP the id Postrinch's own threading gave it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderThreadId(pub String);

/// What a list row shows for a thread, worked out from its messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub account: EmailAddress,
    pub id: ProviderThreadId,
    /// The first message's subject.
    pub subject: String,
    /// Everyone the thread was from or to, each once, earliest first.
    pub participants: Vec<Mailbox>,
    pub latest: Timestamp,
    pub snippet: String,
    pub message_count: usize,
    pub unread_count: usize,
    pub starred: bool,
    pub has_attachments: bool,
    /// Every label any message in the thread carries, each once.
    pub labels: Vec<String>,
    /// Whom the account answered, when its answer is the thread's latest
    /// message: the person is done with it for now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replied_last: Option<Vec<Mailbox>>,
}

/// Whom the account answered, if its reply or reply-all is the latest of
/// `sorted` (oldest first) that is not a draft.
fn replied_last(account: &EmailAddress, sorted: &[&Message]) -> Option<Vec<Mailbox>> {
    let last = sorted.iter().rev().find(|m| !is_draft(m))?;
    answered(account, last)
}

/// Whether a message is a draft, which has not been sent yet.
pub fn is_draft(message: &Message) -> bool {
    message.has_label(SystemLabel::Drafts.provider_id())
}

/// Whom the account answered with `last`, if it is the account's reply or
/// reply-all. A forward passes mail on rather than answering it, and mail
/// only to the account itself answers no one, so neither counts.
pub fn answered(account: &EmailAddress, last: &Message) -> Option<Vec<Mailbox>> {
    let mine = last.has_label(SystemLabel::Sent.provider_id())
        || last
            .headers
            .from
            .iter()
            .any(|f| f.address.key() == account.key());
    let reply = last.headers.in_reply_to.is_some() || !last.headers.references.is_empty();
    let subject = last.subject().trim_start().to_lowercase();
    let forward = subject.starts_with("fwd:") || subject.starts_with("fw:");
    if !mine || !reply || forward {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let others: Vec<Mailbox> = last
        .headers
        .to
        .iter()
        .chain(&last.headers.cc)
        .chain(&last.headers.bcc)
        .filter(|m| m.address.key() != account.key())
        .filter(|m| seen.insert(m.address.key()))
        .cloned()
        .collect();
    (!others.is_empty()).then_some(others)
}

impl ThreadSummary {
    /// Summarize a thread from its messages, in any order. Returns `None`
    /// for no messages. Messages from another account or thread than the
    /// first one are a caller error and are left out.
    pub fn from_messages<'a>(messages: impl IntoIterator<Item = &'a Message>) -> Option<Self> {
        let mut sorted: Vec<&Message> = messages.into_iter().collect();
        let first = *sorted.first()?;
        sorted.retain(|m| m.account == first.account && m.thread == first.thread);
        sorted.sort_by_key(|m| m.received);

        let earliest = sorted[0];
        let latest = sorted[sorted.len() - 1];

        let mut seen = std::collections::HashSet::new();
        let participants = sorted
            .iter()
            .flat_map(|m| m.headers.participants())
            .filter(|m| seen.insert(m.address.key()))
            .cloned()
            .collect();

        let mut labels: Vec<String> = Vec::new();
        for label in sorted.iter().flat_map(|m| &m.labels) {
            if !labels.contains(label) {
                labels.push(label.clone());
            }
        }

        Some(Self {
            account: first.account.clone(),
            id: first.thread.clone(),
            subject: earliest.subject().to_owned(),
            participants,
            latest: latest.received,
            snippet: latest.snippet.clone(),
            message_count: sorted.len(),
            unread_count: sorted.iter().filter(|m| m.flags.unread).count(),
            starred: sorted.iter().any(|m| m.flags.starred),
            has_attachments: sorted.iter().any(|m| m.has_attachments()),
            replied_last: replied_last(&first.account, &sorted),
            labels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Flags, MessageHeaders, ProviderMessageId};

    fn message(id: &str, received: i64, from: &str, unread: bool, labels: &[&str]) -> Message {
        Message {
            account: EmailAddress::parse("me@example.org").unwrap(),
            id: ProviderMessageId(id.into()),
            thread: ProviderThreadId("t1".into()),
            headers: MessageHeaders {
                subject: Some(format!("subject of {id}")),
                from: vec![Mailbox::new(None, EmailAddress::parse(from).unwrap())],
                ..Default::default()
            },
            received: Timestamp::from_millis(received),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            flags: Flags {
                unread,
                starred: false,
            },
            snippet: format!("snippet of {id}"),
            body_text: String::new(),
            downloaded: true,
            attachments: Vec::new(),
            size: 0,
        }
    }

    #[test]
    fn summarizes_in_received_order() {
        let reply = message("m2", 200, "bob@example.org", true, &["INBOX", "Label_1"]);
        let first = message("m1", 100, "ada@example.org", false, &["INBOX"]);
        let summary = ThreadSummary::from_messages([&reply, &first]).unwrap();

        assert_eq!(summary.subject, "subject of m1");
        assert_eq!(summary.snippet, "snippet of m2");
        assert_eq!(summary.latest, Timestamp::from_millis(200));
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.unread_count, 1);
        assert_eq!(summary.labels, ["INBOX", "Label_1"]);
        let who: Vec<_> = summary
            .participants
            .iter()
            .map(|m| m.address.key())
            .collect();
        assert_eq!(who, ["ada@example.org", "bob@example.org"]);
    }

    fn answer(id: &str, received: i64, from: &str, to: &[&str], labels: &[&str]) -> Message {
        let mut m = message(id, received, from, false, labels);
        m.headers.in_reply_to = Some("first@example.org".into());
        m.headers.to = to
            .iter()
            .map(|a| Mailbox::new(None, EmailAddress::parse(a).unwrap()))
            .collect();
        m
    }

    fn whom(messages: &[&Message]) -> Option<Vec<String>> {
        let summary = ThreadSummary::from_messages(messages.iter().copied()).unwrap();
        summary
            .replied_last
            .map(|r| r.iter().map(|m| m.address.key()).collect())
    }

    #[test]
    fn replied_last_when_my_answer_is_latest() {
        let first = message("m1", 100, "bob@example.org", false, &["INBOX"]);
        let to = ["bob@example.org", "me@example.org"];
        let mine = answer("m2", 200, "me@example.org", &to, &[]);
        assert_eq!(whom(&[&first, &mine]), Some(vec!["bob@example.org".into()]));
        // From another of my addresses: Gmail marks it sent.
        let alias = answer("m2", 200, "me.too@example.org", &to, &["SENT"]);
        assert_eq!(
            whom(&[&first, &alias]),
            Some(vec!["bob@example.org".into()])
        );
        // Bob answered after me.
        let again = answer(
            "m3",
            300,
            "bob@example.org",
            &["me@example.org"],
            &["INBOX"],
        );
        assert_eq!(whom(&[&first, &mine, &again]), None);
        // A draft after my answer changes nothing.
        let draft = answer("m3", 300, "me@example.org", &to, &["DRAFT"]);
        assert_eq!(
            whom(&[&first, &mine, &draft]),
            Some(vec!["bob@example.org".into()])
        );
    }

    #[test]
    fn forwards_notes_to_self_and_new_mail_are_not_answers() {
        let first = message("m1", 100, "bob@example.org", false, &["INBOX"]);
        let mut forward = answer("m2", 200, "me@example.org", &["cat@example.org"], &["SENT"]);
        forward.headers.subject = Some("Fwd: the plan".into());
        assert_eq!(whom(&[&first, &forward]), None);
        let to_self = answer("m2", 200, "me@example.org", &["me@example.org"], &["SENT"]);
        assert_eq!(whom(&[&first, &to_self]), None);
        let mut started = answer("m1", 100, "me@example.org", &["bob@example.org"], &["SENT"]);
        started.headers.in_reply_to = None;
        assert_eq!(whom(&[&started]), None);
    }

    #[test]
    fn empty_thread_has_no_summary() {
        assert!(ThreadSummary::from_messages(std::iter::empty()).is_none());
    }
}
