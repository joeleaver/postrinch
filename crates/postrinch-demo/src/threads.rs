//! The demo's conversations, and storing them as sync does: each message
//! written as RFC 5322 text, read back with `postrinch_sync::parse_message`
//! and kept with `Store::put_message`, raw bytes and all.

use postrinch_model::{Flags, ProviderMessageId, ProviderThreadId};
use postrinch_store::Store;
use postrinch_sync::{Envelope, parse_message};

use crate::DemoResult;
use crate::accounts::{IMPORTANT, INBOX, SENT, is_category};
use crate::clock::{At, Clock, timestamp};
use crate::mail::{Outgoing, render};
use crate::people::Person;

/// A conversation in one account.
#[derive(Debug, Clone)]
pub struct ThreadSpec {
    /// The account it is in: whose address it is.
    pub account: Person,
    pub subject: String,
    /// The provider ids of the labels its messages carry (Gmail's way:
    /// `INBOX`, a category, the account's own labels). The account's own
    /// messages carry `SENT` in place of the inbox and its categories.
    pub labels: Vec<&'static str>,
    pub starred: bool,
    pub messages: Vec<MessageSpec>,
}

/// One message of a conversation.
#[derive(Debug, Clone)]
pub struct MessageSpec {
    pub from: Person,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    pub at: At,
    pub body: Body,
    pub attachments: Vec<Attachment>,
    /// An iCalendar `REQUEST`, sent beside the text as invitations are.
    pub invitation: Option<String>,
    pub unread: bool,
}

#[derive(Debug, Clone)]
pub enum Body {
    Text(String),
    /// A newsletter's: HTML, with the text a mail program sends beside it.
    Html {
        text: String,
        html: String,
    },
}

/// A file sent with a message.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: &'static str,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

impl Attachment {
    pub fn new(name: &'static str, mime: &'static str, bytes: &[u8]) -> Self {
        Self {
            name,
            mime,
            bytes: bytes.to_vec(),
        }
    }
}

/// A conversation in `account`'s mail with these labels.
pub fn thread(account: Person, subject: &str, labels: &[&'static str]) -> ThreadSpec {
    ThreadSpec {
        account,
        subject: subject.to_owned(),
        labels: labels.to_vec(),
        starred: false,
        messages: Vec::new(),
    }
}

impl ThreadSpec {
    pub fn starred(mut self) -> Self {
        self.starred = true;
        self
    }

    pub fn msg(mut self, message: MessageSpec) -> Self {
        self.messages.push(message);
        self
    }
}

/// A plain-text message, read.
pub fn msg(from: Person, to: &[Person], at: At, text: &str) -> MessageSpec {
    MessageSpec {
        from,
        to: to.to_vec(),
        cc: Vec::new(),
        at,
        body: Body::Text(text.to_owned()),
        attachments: Vec::new(),
        invitation: None,
        unread: false,
    }
}

impl MessageSpec {
    pub fn unread(mut self) -> Self {
        self.unread = true;
        self
    }

    pub fn cc(mut self, cc: &[Person]) -> Self {
        self.cc = cc.to_vec();
        self
    }

    /// Send `html` as the body, beside the text already given.
    pub fn html(mut self, html: &str) -> Self {
        let text = match self.body {
            Body::Text(text) | Body::Html { text, .. } => text,
        };
        self.body = Body::Html {
            text,
            html: html.to_owned(),
        };
        self
    }

    pub fn attach(mut self, file: Attachment) -> Self {
        self.attachments.push(file);
        self
    }

    pub fn invite(mut self, ics: String) -> Self {
        self.invitation = Some(ics);
        self
    }
}

/// How many conversations and messages were stored.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stored {
    pub threads: usize,
    pub messages: usize,
}

/// Store every conversation. Ids are made as Gmail's look: hex, the
/// thread's id its first message's.
pub fn put_all(
    store: &Store,
    clock: &Clock,
    threads: impl IntoIterator<Item = ThreadSpec>,
) -> DemoResult<Stored> {
    let mut stored = Stored::default();
    for spec in threads {
        stored.messages += put_thread(store, clock, &spec, stored.messages)?;
        stored.threads += 1;
    }
    Ok(stored)
}

/// Store one conversation's messages, oldest first. `seq` numbers
/// messages across the whole store. Returns how many were stored.
fn put_thread(store: &Store, clock: &Clock, spec: &ThreadSpec, seq: usize) -> DemoResult<usize> {
    let mut dated: Vec<_> = spec
        .messages
        .iter()
        .map(|m| (clock.resolve(m.at), m))
        .collect();
    dated.sort_by(|a, b| a.0.cmp(&b.0));
    let account = spec.account.email();
    let mut thread: Option<String> = None;
    let mut earlier: Vec<String> = Vec::new();
    for (n, (date, message)) in dated.iter().enumerate() {
        let received = timestamp(date);
        let id = format!("{:x}{:04x}", received.as_millis(), seq + n);
        let thread_id = thread.get_or_insert_with(|| id.clone()).clone();
        let message_id = format!("{id}.demo@{}", message.from.domain());
        let raw = render(&Outgoing {
            spec: message,
            subject: &reply_subject(&spec.subject, n),
            date,
            message_id: &message_id,
            earlier: &earlier,
        });
        let own = message.from.address == spec.account.address;
        let envelope = Envelope {
            account: account.clone(),
            id: ProviderMessageId(id),
            thread: ProviderThreadId(thread_id),
            received,
            labels: labels_of(&spec.labels, own),
            flags: Flags {
                unread: message.unread && !own,
                starred: spec.starred && n + 1 == dated.len(),
            },
            snippet: None,
        };
        store.put_message(&parse_message(envelope, &raw)?, Some(&raw))?;
        earlier.push(message_id);
    }
    Ok(dated.len())
}

/// A reply's subject: the thread's, after `Re: ` unless it has one.
fn reply_subject(subject: &str, n: usize) -> String {
    let replied = subject
        .get(..3)
        .is_some_and(|start| start.eq_ignore_ascii_case("re:"));
    if n == 0 || replied {
        subject.to_owned()
    } else {
        format!("Re: {subject}")
    }
}

/// The labels one message carries: the thread's, but for the account's
/// own messages, which are in Sent and not in the inbox.
fn labels_of(labels: &[&'static str], own: bool) -> Vec<String> {
    if !own {
        return labels.iter().map(|l| (*l).to_owned()).collect();
    }
    let mut kept: Vec<String> = labels
        .iter()
        .filter(|l| **l != INBOX && **l != IMPORTANT && !is_category(l))
        .map(|l| (*l).to_owned())
        .collect();
    kept.push(SENT.to_owned());
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{FOLLOW_UP, PRIMARY};

    #[test]
    fn own_messages_are_sent_not_in_the_inbox() {
        let thread = [INBOX, PRIMARY, FOLLOW_UP];
        assert_eq!(
            labels_of(&thread, false),
            ["INBOX", "CATEGORY_PERSONAL", FOLLOW_UP]
        );
        assert_eq!(labels_of(&thread, true), [FOLLOW_UP, "SENT"]);
    }

    #[test]
    fn replies_say_so() {
        assert_eq!(reply_subject("Lunch?", 0), "Lunch?");
        assert_eq!(reply_subject("Lunch?", 2), "Re: Lunch?");
        assert_eq!(reply_subject("Re: Offsite", 1), "Re: Offsite");
    }
}
