//! A change the person makes to messages: read, starred, filed.

use serde::{Deserialize, Serialize};

use crate::Flags;

/// What to change about some messages. Labels are named by the provider's
/// id, so archiving is removing the inbox label and trashing is adding the
/// trash label; the provider's sync turns that into its own calls.
///
/// A change is applied here at once and kept until the provider has it, so
/// it can be applied again over whatever sync brings back meanwhile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageChange {
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    /// `Some` sets the flag; `None` leaves it as it is.
    pub unread: Option<bool>,
    pub starred: Option<bool>,
}

impl MessageChange {
    pub fn read() -> Self {
        Self {
            unread: Some(false),
            ..Self::default()
        }
    }

    pub fn unread() -> Self {
        Self {
            unread: Some(true),
            ..Self::default()
        }
    }

    pub fn starred(starred: bool) -> Self {
        Self {
            starred: Some(starred),
            ..Self::default()
        }
    }

    pub fn add_label(label: impl Into<String>) -> Self {
        Self {
            add_labels: vec![label.into()],
            ..Self::default()
        }
    }

    pub fn remove_label(label: impl Into<String>) -> Self {
        Self {
            remove_labels: vec![label.into()],
            ..Self::default()
        }
    }

    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        self.add_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.unread.is_none()
            && self.starred.is_none()
    }

    /// Apply it to a message's labels (kept sorted, each once) and flags.
    pub fn apply(&self, labels: &mut Vec<String>, flags: &mut Flags) {
        labels.retain(|l| !self.remove_labels.contains(l));
        for label in &self.add_labels {
            if !labels.contains(label) {
                labels.push(label.clone());
            }
        }
        labels.sort();
        if let Some(unread) = self.unread {
            flags.unread = unread;
        }
        if let Some(starred) = self.starred {
            flags.starred = starred;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn archiving_removes_the_inbox_label_only() {
        let mut have = labels(&["INBOX", "Label_1"]);
        let mut flags = Flags {
            unread: true,
            starred: false,
        };
        MessageChange::remove_label("INBOX").apply(&mut have, &mut flags);
        assert_eq!(have, labels(&["Label_1"]));
        assert!(flags.unread, "flags left alone");
    }

    #[test]
    fn adding_a_label_twice_keeps_one() {
        let mut have = labels(&["Label_2"]);
        let mut flags = Flags::default();
        let change = MessageChange::add_label("Label_1");
        change.apply(&mut have, &mut flags);
        change.apply(&mut have, &mut flags);
        assert_eq!(have, labels(&["Label_1", "Label_2"]));
    }

    #[test]
    fn flags_set_only_when_named() {
        let mut have = Vec::new();
        let mut flags = Flags {
            unread: true,
            starred: true,
        };
        MessageChange::read().apply(&mut have, &mut flags);
        assert_eq!(
            flags,
            Flags {
                unread: false,
                starred: true
            }
        );
        assert!(MessageChange::default().is_empty());
        assert!(!MessageChange::starred(false).is_empty());
    }
}
