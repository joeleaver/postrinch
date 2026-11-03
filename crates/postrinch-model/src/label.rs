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

/// Whether the provider lists a label (Gmail's `labelListVisibility`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LabelVisibility {
    #[default]
    Show,
    /// Listed only while it holds unread mail.
    ShowIfUnread,
    Hide,
}

/// A label's colours as the provider has them, `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LabelColor {
    pub background: String,
    pub text: String,
}

/// What the person changed about a label on this computer. None of it is
/// sent to the provider, and sync never touches it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LabelPrefs {
    /// A name to show instead of the provider's.
    #[serde(default)]
    pub name: Option<String>,
    /// Shown or hidden whatever the defaults say; `None` follows them.
    #[serde(default)]
    pub shown: Option<bool>,
    /// Listed with the folders at the top of the sidebar.
    #[serde(default)]
    pub pinned: bool,
    /// A colour to use instead of the provider's, `#rrggbb`.
    #[serde(default)]
    pub color: Option<String>,
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
    #[serde(default)]
    pub visibility: LabelVisibility,
    #[serde(default)]
    pub color: Option<LabelColor>,
}

impl Label {
    /// A label the provider lists, in no particular colour.
    pub fn new(provider_id: impl Into<String>, name: impl Into<String>, kind: LabelKind) -> Self {
        Self {
            provider_id: provider_id.into(),
            name: name.into(),
            kind,
            visibility: LabelVisibility::Show,
            color: None,
        }
    }

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
        let label = Label::new("Label_7", "Work/Clients/Acme", LabelKind::User);
        assert_eq!(
            label.path().collect::<Vec<_>>(),
            ["Work", "Clients", "Acme"]
        );
        assert!(!label.is_system());
    }
}
