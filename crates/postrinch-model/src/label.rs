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

impl SystemLabel {
    /// Every system label, in the order above.
    pub const ALL: [SystemLabel; 11] = [
        Self::Inbox,
        Self::Sent,
        Self::Drafts,
        Self::Spam,
        Self::Trash,
        Self::Important,
        Self::CategoryPersonal,
        Self::CategorySocial,
        Self::CategoryPromotions,
        Self::CategoryUpdates,
        Self::CategoryForums,
    ];

    /// The label's provider id, the same in every account of every
    /// provider. This is a contract: these are Gmail's own ids for its
    /// system labels, and an IMAP account's special folders take them too
    /// (its Sent folder is `SENT` whatever the server calls it). So
    /// "archive is removing `INBOX`" and "trash is adding `TRASH`" hold for
    /// every account, and code naming a system label uses this rather than
    /// a string of its own.
    pub const fn provider_id(&self) -> &'static str {
        match self {
            Self::Inbox => "INBOX",
            Self::Sent => "SENT",
            Self::Drafts => "DRAFT",
            Self::Spam => "SPAM",
            Self::Trash => "TRASH",
            Self::Important => "IMPORTANT",
            Self::CategoryPersonal => "CATEGORY_PERSONAL",
            Self::CategorySocial => "CATEGORY_SOCIAL",
            Self::CategoryPromotions => "CATEGORY_PROMOTIONS",
            Self::CategoryUpdates => "CATEGORY_UPDATES",
            Self::CategoryForums => "CATEGORY_FORUMS",
        }
    }

    /// The system label whose [`provider_id`](Self::provider_id) is `id`,
    /// if any.
    pub fn from_provider_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|label| label.provider_id() == id)
    }

    /// Whether this is one of Gmail's inbox categories.
    pub fn is_category(&self) -> bool {
        matches!(
            self,
            Self::CategoryPersonal
                | Self::CategorySocial
                | Self::CategoryPromotions
                | Self::CategoryUpdates
                | Self::CategoryForums
        )
    }
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
/// Gmail lets a message carry many labels; an IMAP message is in the folders
/// that hold a copy of it. The model treats both as labels, so an IMAP
/// message has one label for each folder it is in (docs/imap.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    /// The provider's identifier: Gmail's label id (`Label_12`, `INBOX`), or
    /// the id Postrinch minted for an IMAP user folder (`f` and 12 hex
    /// digits). A system label's is always [`SystemLabel::provider_id`].
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

    #[test]
    fn provider_ids_are_gmails_and_round_trip() {
        assert_eq!(SystemLabel::Inbox.provider_id(), "INBOX");
        assert_eq!(SystemLabel::Drafts.provider_id(), "DRAFT");
        assert_eq!(
            SystemLabel::CategoryPersonal.provider_id(),
            "CATEGORY_PERSONAL"
        );
        for label in SystemLabel::ALL {
            assert_eq!(
                SystemLabel::from_provider_id(label.provider_id()),
                Some(label)
            );
        }
        assert_eq!(SystemLabel::from_provider_id("UNREAD"), None);
        assert_eq!(SystemLabel::from_provider_id("Label_1"), None);
    }
}
