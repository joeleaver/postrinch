use serde::{Deserialize, Serialize};

use crate::{EmailAddress, Mailbox, ProviderThreadId, Timestamp};

/// The provider's identifier for a message within one account: Gmail's
/// message id, or an IMAP folder's UIDVALIDITY and UID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProviderMessageId(pub String);

/// The headers Postrinch reads, parsed. The raw message is kept apart and
/// goes to the renderer; these are what lists, threads and search use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageHeaders {
    /// `Message-ID`, without its angle brackets.
    pub message_id: Option<String>,
    /// `In-Reply-To`, without angle brackets.
    pub in_reply_to: Option<String>,
    /// `References`, oldest first, without angle brackets.
    pub references: Vec<String>,
    pub subject: Option<String>,
    pub from: Vec<Mailbox>,
    pub sender: Option<Mailbox>,
    pub reply_to: Vec<Mailbox>,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    /// The `Date:` header, which the sender's machine wrote and may be
    /// wrong. [`Message::received`] is the time to sort by.
    pub date: Option<Timestamp>,
}

impl MessageHeaders {
    /// Every address the message was from or to, each once, in header order.
    pub fn participants(&self) -> Vec<&Mailbox> {
        let mut seen = std::collections::HashSet::new();
        self.from
            .iter()
            .chain(&self.to)
            .chain(&self.cc)
            .chain(&self.bcc)
            .filter(|m| seen.insert(m.address.key()))
            .collect()
    }
}

/// Strip the angle brackets and whitespace from a `Message-ID`-style value.
/// Returns `None` for a value that is empty once stripped.
pub(crate) fn normalize_message_id(raw: &str) -> Option<String> {
    let id = raw
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    (!id.is_empty()).then(|| id.to_owned())
}

impl MessageHeaders {
    pub fn set_message_id(&mut self, raw: &str) {
        self.message_id = normalize_message_id(raw);
    }

    pub fn set_in_reply_to(&mut self, raw: &str) {
        self.in_reply_to = raw.split_whitespace().find_map(normalize_message_id);
    }

    pub fn set_references(&mut self, raw: &str) {
        self.references = raw
            .split_whitespace()
            .filter_map(normalize_message_id)
            .collect();
    }
}

/// Read and starred, the two flags every provider has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flags {
    pub unread: bool,
    pub starred: bool,
}

/// An attachment, described; its bytes stay with the raw message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub filename: Option<String>,
    pub mime_type: String,
    pub size: u64,
    /// `Content-ID`, for an inline part an HTML body refers to as `cid:`.
    pub content_id: Option<String>,
    pub inline: bool,
}

/// One message in one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub account: EmailAddress,
    pub id: ProviderMessageId,
    pub thread: ProviderThreadId,
    pub headers: MessageHeaders,
    /// When the provider received it (Gmail's `internalDate`, IMAP's
    /// `INTERNALDATE`).
    pub received: Timestamp,
    /// The `provider_id`s of the labels it carries.
    pub labels: Vec<String>,
    pub flags: Flags,
    /// A short preview, as the provider gives it or taken from the body.
    pub snippet: String,
    /// The body as plain text, for search and for an agent: the
    /// `text/plain` part, or the text of the HTML part when there is none.
    pub body_text: String,
    pub attachments: Vec<AttachmentInfo>,
    /// The raw message's size in bytes (the provider's estimate until it
    /// is downloaded).
    pub size: u64,
    /// Whether the whole message has been downloaded. Until it has, only
    /// the headers, labels, flags and snippet are known: `body_text` is
    /// empty and `attachments` lists nothing, whatever the message holds.
    pub downloaded: bool,
}

impl Message {
    pub fn subject(&self) -> &str {
        self.headers.subject.as_deref().unwrap_or("")
    }

    pub fn has_label(&self, provider_id: &str) -> bool {
        self.labels.iter().any(|l| l == provider_id)
    }

    pub fn has_attachments(&self) -> bool {
        self.attachments.iter().any(|a| !a.inline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox(addr: &str) -> Mailbox {
        Mailbox::new(None, EmailAddress::parse(addr).unwrap())
    }

    #[test]
    fn message_ids_lose_brackets() {
        let mut h = MessageHeaders::default();
        h.set_message_id("  <abc@mail.example.org> ");
        h.set_in_reply_to("<parent@example.org>");
        h.set_references("<a@x> <b@x>\r\n <c@x>");
        assert_eq!(h.message_id.as_deref(), Some("abc@mail.example.org"));
        assert_eq!(h.in_reply_to.as_deref(), Some("parent@example.org"));
        assert_eq!(h.references, ["a@x", "b@x", "c@x"]);

        h.set_message_id("<>");
        assert_eq!(h.message_id, None);
    }

    #[test]
    fn participants_are_unique_by_key() {
        let h = MessageHeaders {
            from: vec![mailbox("Ada@example.org")],
            to: vec![mailbox("ada@EXAMPLE.org"), mailbox("bob@example.org")],
            cc: vec![mailbox("bob@example.org")],
            ..Default::default()
        };
        let keys: Vec<_> = h.participants().iter().map(|m| m.address.key()).collect();
        assert_eq!(keys, ["ada@example.org", "bob@example.org"]);
    }
}
