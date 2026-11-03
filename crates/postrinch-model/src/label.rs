use serde::{Deserialize, Serialize};

/// A label every account has, whatever the provider calls it: Gmail's
/// `INBOX` label, an IMAP server's `INBOX` folder or its `\Sent` mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SystemLabel {
    Inbox,
    Sent,
    Drafts,
    Spam,
    Trash,
    Important,
    /// Gmail's inbox categories.
    CategoryPersonal,
    CategorySocial,
    CategoryPromotions,
    CategoryUpdates,
    CategoryForums,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelKind {
    System(SystemLabel),
    /// A label or folder the person made.
    User,
}

/// A Gmail label or an IMAP folder, in one account.
///
/// Gmail lets a message carry many labels; an IMAP message is in exactly one
/// folder. The model treats both as labels, and an IMAP message simply has
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    /// The provider's identifier: Gmail's label id (`Label_12`, `INBOX`), or
    /// the IMAP folder's full path.
    pub provider_id: String,
    /// The name the person sees. For nested labels, the full path
    /// (`Work/Clients`).
    pub name: String,
    pub kind: LabelKind,
}

impl Label {
    pub fn is_system(&self) -> bool {
        matches!(self.kind, LabelKind::System(_))
    }

    /// The path segments of a nested label's name.
    pub fn path(&self) -> impl Iterator<Item = &str> {
        self.name.split('/').filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_path() {
        let label = Label {
            provider_id: "Label_7".into(),
            name: "Work/Clients/Acme".into(),
            kind: LabelKind::User,
        };
        assert_eq!(
            label.path().collect::<Vec<_>>(),
            ["Work", "Clients", "Acme"]
        );
        assert!(!label.is_system());
    }
}
