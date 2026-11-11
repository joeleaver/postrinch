//! How conversations show, as the person set it in Settings › Reading:
//! as conversations or one message at a time, newest or oldest at the top,
//! and which messages open in full when a conversation opens (the others
//! fold to one line). Kept in Postrinch's settings file (see appearance.rs).

use std::collections::HashSet;

use postrinch_commands::Thread;
use postrinch_model::{Message, ProviderThreadId};
use rinch::{Effect, untracked};
use serde::{Deserialize, Serialize};

use crate::state::State;

/// Whether the list shows conversations or messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShowMail {
    /// A message and its replies are one row, read together.
    #[default]
    Conversations,
    /// Every message is its own row, and an action changes only it.
    Messages,
}

/// Which end of a conversation is at the top.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Order {
    #[default]
    NewestFirst,
    OldestFirst,
}

/// Which of a conversation's messages open in full; the rest fold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenInFull {
    #[default]
    UnreadAndNewest,
    Newest,
    All,
}

/// The messages open in full in the open conversation: chosen by the
/// setting when the conversation opens, then by the person's clicks. Kept
/// for the conversation it was chosen for, so marking it read on open does
/// not fold what was unread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opened {
    pub thread: Option<ProviderThreadId>,
    pub messages: HashSet<String>,
}

/// The messages `rule` opens in full, of a thread's messages (oldest first).
pub fn opened_by(rule: OpenInFull, messages: &[Message]) -> HashSet<String> {
    let newest = messages.iter().max_by_key(|m| m.received).map(|m| &m.id);
    messages
        .iter()
        .filter(|m| match rule {
            OpenInFull::All => true,
            OpenInFull::Newest => Some(&m.id) == newest,
            OpenInFull::UnreadAndNewest => m.flags.unread || Some(&m.id) == newest,
        })
        .map(|m| m.id.0.clone())
        .collect()
}

/// A thread's messages in the order the setting shows them.
pub fn in_order(order: Order, thread: &Thread) -> Vec<Message> {
    let mut messages = thread.messages.clone();
    messages.sort_by_key(|m| m.received);
    if order == Order::NewestFirst {
        messages.reverse();
    }
    messages
}

impl State {
    /// Choose which messages open in full whenever another conversation
    /// opens.
    pub fn open_by_setting(self) {
        Effect::new(move || {
            let Some(thread) = self.thread.get() else {
                return;
            };
            let id = Some(thread.summary.id.clone());
            if untracked(|| self.opened.with(|o| o.thread == id)) {
                return;
            }
            let rule = untracked(|| self.open_in_full.get());
            self.opened.set(Opened {
                thread: id,
                messages: opened_by(rule, &thread.messages),
            });
        });
    }

    /// Whether a message of the open conversation shows in full.
    pub fn is_open(self, message: &Message) -> bool {
        self.opened.with(|o| o.messages.contains(&message.id.0))
    }

    /// Open one folded message in full.
    pub fn open_message(self, message: &Message) {
        let id = message.id.0.clone();
        self.opened.update(|o| {
            o.messages.insert(id);
        });
    }

    /// Open every message of the open conversation in full.
    pub fn open_all(self) {
        let Some(thread) = self.thread.get() else {
            return;
        };
        self.opened.update(|o| {
            o.messages = opened_by(OpenInFull::All, &thread.messages);
        });
    }

    pub fn set_show_mail(self, choice: ShowMail) {
        self.show_mail.set(choice);
        self.save_settings(|s| s.show_mail = choice);
    }

    pub fn set_order(self, choice: Order) {
        self.order.set(choice);
        self.save_settings(|s| s.order = choice);
    }

    pub fn set_open_in_full(self, choice: OpenInFull) {
        self.open_in_full.set(choice);
        self.save_settings(|s| s.open_in_full = choice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postrinch_model::{EmailAddress, Flags, MessageHeaders, ProviderMessageId, Timestamp};

    fn message(id: &str, at: i64, unread: bool) -> Message {
        Message {
            account: EmailAddress::parse("me@example.com").unwrap(),
            id: ProviderMessageId(id.into()),
            thread: ProviderThreadId("t".into()),
            headers: MessageHeaders::default(),
            received: Timestamp::from_millis(at),
            labels: Vec::new(),
            flags: Flags {
                unread,
                ..Flags::default()
            },
            snippet: String::new(),
            body_text: String::new(),
            attachments: Vec::new(),
            size: 0,
            downloaded: true,
        }
    }

    fn ids(set: HashSet<String>) -> Vec<String> {
        let mut ids: Vec<String> = set.into_iter().collect();
        ids.sort();
        ids
    }

    #[test]
    fn unread_and_the_newest_open_the_rest_fold() {
        let messages = [
            message("a", 1, false),
            message("b", 2, true),
            message("c", 3, false),
        ];
        assert_eq!(
            ids(opened_by(OpenInFull::UnreadAndNewest, &messages)),
            ["b", "c"]
        );
        assert_eq!(ids(opened_by(OpenInFull::Newest, &messages)), ["c"]);
        assert_eq!(ids(opened_by(OpenInFull::All, &messages)), ["a", "b", "c"]);
    }
}
