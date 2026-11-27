//! What Social mode says about views and places, after the Social mockup
//! (design/mockups/ofscp-social.html): each view's mark and count in the
//! sidebar, the line under a view's name, its tabs, and how a place or
//! group tells the person of new messages. Plain functions of what the
//! commands give, so they are tested without a window.

use postrinch_commands::{BuiltInView, SocialPlaceInfo, SocialViewEntry, SocialViewTally};
use postrinch_store::{ContentKind, Notices, PlaceKind, PlaceRef, ReadAs};

use crate::ui::account_color;

/// A group as the sidebar marks a view with it: its initial on its colour.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupMark {
    pub uri: String,
    pub name: String,
}

impl GroupMark {
    /// The group's initial, as its square shows it.
    pub fn initial(&self) -> String {
        self.name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "#".to_owned())
    }

    /// The group's colour, the same wherever it shows: one of the
    /// accounts' colours, picked by its URI.
    pub fn color(&self) -> &'static str {
        // FNV-1a: stable from one run to the next, unlike std's hasher.
        let hash = self.uri.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        account_color((hash % 5) as usize)
    }
}

/// What stands before a view's name in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// A glyph in the muted colour.
    Glyph(&'static str),
    /// A view of one group: the group's square.
    Group(GroupMark),
    /// A view of several groups: theirs, overlapping.
    Stack(Vec<GroupMark>),
}

/// How many groups a stack shows at most.
const STACK: usize = 3;

/// A view's mark: a built-in view's glyph; ▤ for a view read as a list;
/// otherwise the groups its places are in.
pub fn view_mark(entry: &SocialViewEntry, groups: &[GroupMark]) -> Mark {
    if let Some(kind) = entry.built_in {
        return Mark::Glyph(match kind {
            BuiltInView::Home => "\u{2302}",
            BuiltInView::Mentions => "@",
            BuiltInView::Messages => "\u{2709}",
            BuiltInView::Drafts => "\u{270e}",
            BuiltInView::AllChannels => "#",
        });
    }
    if entry.view.read_as == ReadAs::List {
        return Mark::Glyph("\u{25a4}");
    }
    match groups {
        [] => Mark::Glyph("#"),
        [one] => Mark::Group(one.clone()),
        many => Mark::Stack(many.iter().take(STACK).cloned().collect()),
    }
}

/// The groups a view is marked by in the sidebar: the whole groups it
/// names, when it names any (a view of a group, with a channel from
/// elsewhere too, is that group's view); else the groups of its places.
pub fn marking_groups(entry: &SocialViewEntry, places: &[SocialPlaceInfo]) -> Vec<GroupMark> {
    let all = groups_of(places);
    if entry.view.groups.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|g| entry.view.groups.iter().any(|named| named.uri == g.uri))
        .collect()
}

/// The groups of a view's places, each once, in the places' order.
pub fn groups_of(places: &[SocialPlaceInfo]) -> Vec<GroupMark> {
    let mut out: Vec<GroupMark> = Vec::new();
    for info in places {
        if let (Some(uri), Some(name)) = (&info.place.group, &info.group_name)
            && !out.iter().any(|g| &g.uri == uri)
        {
            out.push(GroupMark {
                uri: uri.clone(),
                name: name.clone(),
            });
        }
    }
    out
}

/// The number beside a view in the sidebar, as its count asks, and whether
/// it stands out as a badge: when unread messages mention the person, or
/// in Messages, where every message is to them. `None` for nothing to
/// count.
pub fn row_count(
    entry: &SocialViewEntry,
    tally: Option<&SocialViewTally>,
) -> Option<(usize, bool)> {
    let tally = tally?;
    let n = tally.shown(entry.view.count).filter(|&n| n > 0)?;
    let loud = tally.mentions > 0 || entry.built_in == Some(BuiltInView::Messages);
    Some((n, loud))
}

/// The line after a view's name in its header: Home's followed channels;
/// the kinds a view keeps to and from how many places; the provider of a
/// view of one group; or how many places on how many providers. Nothing
/// for a view of no places.
pub fn sub_line(entry: &SocialViewEntry, places: &[SocialPlaceInfo]) -> String {
    let n = places.len();
    match entry.built_in {
        Some(BuiltInView::Home) => {
            return format!(
                "posts from {n} followed {}",
                plural(n, "channel", "channels")
            );
        }
        Some(_) => return String::new(),
        None => {}
    }
    if n == 0 {
        return String::new();
    }
    let kinds = &entry.view.filters.kinds;
    if !kinds.is_empty() && kinds.len() < 3 {
        let named: Vec<&str> = kinds.iter().map(|&k| kind_plural(k)).collect();
        return format!(
            "{} from {n} {}",
            named.join(" and "),
            plural(n, "place", "places")
        );
    }
    let groups = groups_of(places);
    let providers = providers(places);
    if groups.len() == 1 && places.iter().all(|p| p.place.kind == PlaceKind::Channel) {
        return providers.into_iter().next().unwrap_or_default();
    }
    let m = providers.len();
    format!(
        "{n} {} on {m} {}",
        plural(n, "place", "places"),
        plural(m, "provider", "providers")
    )
}

/// The providers holding `places`, each once: a channel's is its URI's
/// authority; a conversation's, its account's domain.
fn providers(places: &[SocialPlaceInfo]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for info in places {
        let domain = place_provider(info);
        if !out.contains(&domain) {
            out.push(domain);
        }
    }
    out
}

/// The provider a place is on.
pub fn place_provider(info: &SocialPlaceInfo) -> String {
    let place = &info.place;
    let from_uri = |uri: &str| {
        uri.strip_prefix("https://")
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .filter(|a| !a.is_empty())
            .map(str::to_ascii_lowercase)
    };
    match place.kind {
        PlaceKind::Channel => from_uri(place.group.as_deref().unwrap_or(&place.id)),
        PlaceKind::Conversation => None,
    }
    .unwrap_or_else(|| account_domain(&place.account))
}

/// The domain of an account (`handle@domain`).
pub fn account_domain(actor: &str) -> String {
    actor
        .rsplit_once('@')
        .map_or(actor, |(_, d)| d)
        .to_ascii_lowercase()
}

/// Whether a view's header shows its places as tabs: one of the person's
/// own views, read as a stream, with more than one place. Built-in views
/// gather from everywhere (Messages, Mentions), so they have no tabs.
pub fn shows_tabs(entry: &SocialViewEntry, places: &[SocialPlaceInfo]) -> bool {
    entry.built_in.is_none() && entry.view.read_as == ReadAs::Stream && places.len() > 1
}

/// The tab chosen, while it still is one: the view shows tabs and the
/// place is still among its places. Else "All".
pub fn chosen_tab(
    entry: &SocialViewEntry,
    places: &[SocialPlaceInfo],
    tab: Option<&PlaceRef>,
) -> Option<PlaceRef> {
    let tab = tab?;
    (shows_tabs(entry, places) && places.iter().any(|p| crate::social_ui::is_place(p, tab)))
        .then(|| tab.clone())
}

/// Whether a view's tabs say which group each place is in: when its
/// places are in more than one.
pub fn tabs_say_where(places: &[SocialPlaceInfo]) -> bool {
    groups_of(places).len() > 1
}

/// The glyph before a place's name: ◖ for a call channel, ▤ for a channel
/// nobody may chat in (posts only: a blog, announcements), # for other
/// channels, ✉ for a private conversation.
pub fn place_glyph(info: &SocialPlaceInfo) -> &'static str {
    let object = &info.place.object;
    if info.place.kind == PlaceKind::Conversation {
        return "\u{2709}";
    }
    if object.get("type").and_then(|t| t.as_str()) == Some("call") {
        return "\u{25d6}";
    }
    let no_chat = object
        .pointer("/permissions/post:message")
        .and_then(|roles| roles.as_array())
        .is_some_and(Vec::is_empty);
    if no_chat { "\u{25a4}" } else { "#" }
}

/// How a place or group tells the person of new messages, as its chip
/// says it. `None` is a channel's group's setting, or a group's
/// provider's default.
pub fn notices_name(notices: Option<Notices>, of_group: bool) -> &'static str {
    match notices {
        Some(Notices::Everything) => "everything",
        Some(Notices::MentionsAndReplies) => "mentions and replies",
        Some(Notices::Nothing) => "nothing",
        None if of_group => "the provider's default",
        None => "as its group",
    }
}

/// The choices a notices menu offers, in order.
pub fn notices_choices(of_group: bool) -> [(Option<Notices>, &'static str); 4] {
    [
        (Some(Notices::Everything), "Everything"),
        (Some(Notices::MentionsAndReplies), "Mentions and replies"),
        (Some(Notices::Nothing), "Nothing"),
        (
            None,
            if of_group {
                "The provider's default"
            } else {
                "As its group"
            },
        ),
    ]
}

/// What a built-in view shows, said in its panel in place of its places.
pub fn built_in_holds(kind: BuiltInView) -> &'static str {
    match kind {
        BuiltInView::Home => "Posts from the channels you follow.",
        BuiltInView::Mentions => "Messages that mention you, in every place.",
        BuiltInView::Messages => "Your private conversations.",
        BuiltInView::Drafts => "What you are writing.",
        BuiltInView::AllChannels => "Every place Postrinch knows.",
    }
}

fn kind_plural(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Chat => "chat",
        ContentKind::Memo => "memos",
        ContentKind::Article => "articles",
    }
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

#[cfg(test)]
#[path = "social_shown_tests.rs"]
mod tests;
