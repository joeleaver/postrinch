use serde::{Deserialize, Serialize};

use crate::{EmailAddress, Mailbox, Message, Timestamp};

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

    #[test]
    fn empty_thread_has_no_summary() {
        assert!(ThreadSummary::from_messages(std::iter::empty()).is_none());
    }
}
