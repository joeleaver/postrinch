//! The built-in social views (docs/ofscp.md "Organizing"): Home, Mentions,
//! Messages and Drafts first, All channels at the foot. What each shows is
//! fixed; how it shows it (its name, filters, how it reads, its count) can
//! be saved over and put back, as mail's built-in views can.

use postrinch_store::{ContentKind, ReadAs, SocialCount, SocialFilters, SocialView};
use serde::{Deserialize, Serialize};

pub const HOME: &str = "home";
pub const MENTIONS: &str = "mentions";
pub const MESSAGES: &str = "messages";
pub const DRAFTS: &str = "drafts";
pub const ALL_CHANNELS: &str = "all-channels";

/// What a built-in view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltInView {
    /// Posts from the channels the person follows.
    Home,
    /// Messages that mention the person, in every place.
    Mentions,
    /// Private conversations.
    Messages,
    /// What the person is writing.
    Drafts,
    /// Every place Postrinch knows, to browse, join and follow from.
    AllChannels,
}

impl BuiltInView {
    /// Those listed before the person's own views, in the sidebar's order.
    pub const FIRST: [Self; 4] = [Self::Home, Self::Mentions, Self::Messages, Self::Drafts];

    /// Its view id.
    pub fn id(self) -> &'static str {
        match self {
            Self::Home => HOME,
            Self::Mentions => MENTIONS,
            Self::Messages => MESSAGES,
            Self::Drafts => DRAFTS,
            Self::AllChannels => ALL_CHANNELS,
        }
    }

    /// The built-in view with this id.
    pub fn of(id: &str) -> Option<Self> {
        Self::FIRST
            .into_iter()
            .chain([Self::AllChannels])
            .find(|v| v.id() == id)
    }

    /// Whether it shows messages, which filters can narrow: Drafts lists
    /// drafts, and All channels places.
    pub fn shows_messages(self) -> bool {
        matches!(self, Self::Home | Self::Mentions | Self::Messages)
    }

    /// The view as it first was.
    pub fn original(self) -> SocialView {
        let (name, kinds, read_as, count) = match self {
            Self::Home => (
                "Home",
                vec![ContentKind::Memo, ContentKind::Article],
                ReadAs::List,
                SocialCount::Unread,
            ),
            Self::Mentions => ("Mentions", Vec::new(), ReadAs::Stream, SocialCount::Unread),
            Self::Messages => ("Messages", Vec::new(), ReadAs::Stream, SocialCount::Unread),
            Self::Drafts => ("Drafts", Vec::new(), ReadAs::List, SocialCount::None),
            Self::AllChannels => ("All channels", Vec::new(), ReadAs::List, SocialCount::None),
        };
        SocialView {
            id: self.id().to_owned(),
            order: 0,
            name: name.to_owned(),
            places: Vec::new(),
            groups: Vec::new(),
            filters: SocialFilters {
                kinds,
                search: String::new(),
            },
            read_as,
            count,
        }
    }
}
