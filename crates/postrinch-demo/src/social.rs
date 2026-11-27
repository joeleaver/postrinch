//! The demo person's social account, after the Social mockup: Sam on
//! forumall.example, a member of Rust Audio and Book club there, of OFSCP Spec
//! on ofscp.example and of Synth Makers on sonic.example, with private
//! conversations with Mira, Ada and Tomás; a few unread messages in each,
//! two that mention Sam; and Sam's own views (Rust Audio, Spec work, Long
//! reads, Book club). Kept as sync keeps them, so the window's Social mode
//! shows them as it would a synced account's.

use jiff::Zoned;
use postrinch_commands::Commands;
use postrinch_store::{
    ContentKind, GroupFound, GroupRef, Notices, PlaceCursors, PlaceKind, PlaceRef, ReadAs,
    SocialAccount, SocialAccountState, SocialCount, SocialFilters, SocialGroup, SocialMessage,
    SocialPlace, SocialView, Store,
};
use serde_json::{Value, json};

use crate::DemoResult;

/// The demo person's social account.
pub const SAM: &str = "sam@forumall.example";

const RUST: &str = "https://forumall.example/api/groups/rust-audio";
const BOOKS: &str = "https://forumall.example/api/groups/book-club";
const SPEC: &str = "https://ofscp.example/api/groups/spec";
const SYNTH: &str = "https://sonic.example/api/groups/synth-makers";

/// What a channel is for, which its glyph shows.
#[derive(Clone, Copy)]
enum Channel {
    /// Everyone chats.
    Chat,
    /// Only articles, from the group's writers.
    Articles,
    /// A voice call.
    Call,
}

/// A channel: its group, its name, what it is for, whether Sam follows it.
struct ChannelSpec {
    group: &'static str,
    name: &'static str,
    channel: Channel,
    followed: bool,
}

const CHANNELS: [ChannelSpec; 8] = [
    ChannelSpec {
        group: RUST,
        name: "general",
        channel: Channel::Chat,
        followed: true,
    },
    ChannelSpec {
        group: RUST,
        name: "announcements",
        channel: Channel::Articles,
        followed: true,
    },
    ChannelSpec {
        group: RUST,
        name: "standup",
        channel: Channel::Call,
        followed: true,
    },
    ChannelSpec {
        group: RUST,
        name: "ofscp",
        channel: Channel::Chat,
        followed: true,
    },
    ChannelSpec {
        group: SPEC,
        name: "chat",
        channel: Channel::Chat,
        followed: true,
    },
    ChannelSpec {
        group: SPEC,
        name: "proposals",
        channel: Channel::Articles,
        followed: true,
    },
    ChannelSpec {
        group: SYNTH,
        name: "blog",
        channel: Channel::Articles,
        followed: true,
    },
    ChannelSpec {
        group: BOOKS,
        name: "general",
        channel: Channel::Chat,
        followed: false,
    },
];

/// A message: where, who, what kind, how long ago, whether it mentions
/// Sam, whether Sam has read it, and what it says.
struct MessageSpec {
    place: &'static str,
    author: &'static str,
    kind: &'static str,
    minutes_ago: i64,
    mentions_me: bool,
    unread: bool,
    text: &'static str,
}

const MIRA: &str = "mira@forumall.example";
const ADA: &str = "ada@ofscp.example";
const TOMAS: &str = "tomas@sonic.example";

/// Places are named here by group and channel ("rust/general") or by
/// conversation ("dm_mira"), and found by [`place_id`].
const MESSAGES: [MessageSpec; 14] = [
    MessageSpec {
        place: "rust/general",
        author: MIRA,
        kind: "message",
        minutes_ago: 30,
        mentions_me: false,
        unread: true,
        text: "The new resampler is in. Benchmarks next.",
    },
    MessageSpec {
        place: "rust/general",
        author: TOMAS,
        kind: "message",
        minutes_ago: 22,
        mentions_me: true,
        unread: true,
        text: "@sam can you share the numbers from this morning?",
    },
    MessageSpec {
        place: "rust/general",
        author: MIRA,
        kind: "message",
        minutes_ago: 12,
        mentions_me: false,
        unread: true,
        text: "Rack photos from the bench, for the curious.",
    },
    MessageSpec {
        place: "rust/general",
        author: TOMAS,
        kind: "message",
        minutes_ago: 300,
        mentions_me: false,
        unread: false,
        text: "Morning all.",
    },
    MessageSpec {
        place: "rust/announcements",
        author: MIRA,
        kind: "article",
        minutes_ago: 120,
        mentions_me: false,
        unread: false,
        text: "Meetup on 4 December: library again, doors at six.",
    },
    MessageSpec {
        place: "rust/ofscp",
        author: ADA,
        kind: "message",
        minutes_ago: 3000,
        mentions_me: false,
        unread: false,
        text: "The remote groups proposal is up for comments.",
    },
    MessageSpec {
        place: "spec/chat",
        author: ADA,
        kind: "message",
        minutes_ago: 90,
        mentions_me: true,
        unread: true,
        text: "@sam left a comment on your proposal.",
    },
    MessageSpec {
        place: "spec/proposals",
        author: ADA,
        kind: "article",
        minutes_ago: 2900,
        mentions_me: false,
        unread: true,
        text: "Membership pointers for remote groups",
    },
    MessageSpec {
        place: "spec/proposals",
        author: ADA,
        kind: "article",
        minutes_ago: 4400,
        mentions_me: false,
        unread: true,
        text: "Read markers across devices",
    },
    MessageSpec {
        place: "spec/proposals",
        author: MIRA,
        kind: "article",
        minutes_ago: 5800,
        mentions_me: false,
        unread: true,
        text: "Calls in text channels",
    },
    MessageSpec {
        place: "spec/proposals",
        author: TOMAS,
        kind: "article",
        minutes_ago: 7200,
        mentions_me: false,
        unread: true,
        text: "Media that expires",
    },
    MessageSpec {
        place: "synth/blog",
        author: TOMAS,
        kind: "article",
        minutes_ago: 60,
        mentions_me: false,
        unread: true,
        text: "Building a ladder filter in Rust",
    },
    MessageSpec {
        place: "dm_mira",
        author: MIRA,
        kind: "message",
        minutes_ago: 8,
        mentions_me: false,
        unread: true,
        text: "Thanks! Can you send the config too?",
    },
    MessageSpec {
        place: "dm_ada",
        author: ADA,
        kind: "message",
        minutes_ago: 2800,
        mentions_me: false,
        unread: false,
        text: "Left a comment on your proposal.",
    },
];

/// Keep the demo social account, its groups, places, messages and views;
/// the number of views of Sam's own.
pub fn put(store: &Store, commands: &Commands, now: &Zoned) -> DemoResult<usize> {
    store.put_social_account(&SocialAccount {
        actor: SAM.into(),
        state: SocialAccountState::Active,
        profile: Some(json!({ "handle": "sam", "displayName": "Sam Calloway" })),
        settings: None,
        key_id: Some("dk_demo".into()),
    })?;
    for (uri, name, notices) in [
        (RUST, "Rust Audio", Some(Notices::MentionsAndReplies)),
        (BOOKS, "Book club", None),
        (SPEC, "OFSCP Spec", None),
        (SYNTH, "Synth Makers", Some(Notices::Everything)),
    ] {
        store.put_social_group(&group(uri, name, notices))?;
    }
    for spec in &CHANNELS {
        store.put_social_place(&channel(spec))?;
    }
    for with in [MIRA, ADA, TOMAS] {
        store.put_social_place(&conversation(with))?;
    }
    let now_ms = now.timestamp().as_millisecond();
    for (n, spec) in MESSAGES.iter().enumerate() {
        let message = message(spec, n, now_ms);
        store.put_social_message(&message, spec.text)?;
    }
    let views = own_views();
    for view in &views {
        commands.save_social_view(view)?;
    }
    Ok(views.len())
}

fn group(uri: &str, name: &str, notices: Option<Notices>) -> SocialGroup {
    SocialGroup {
        account: SAM.into(),
        uri: uri.into(),
        provider: provider_of(uri).into(),
        group: json!({ "id": uri, "name": name }),
        role: Some("member".into()),
        found: GroupFound::Member,
        notices,
    }
}

fn provider_of(uri: &str) -> &str {
    uri.trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or_default()
}

fn channel_id(group: &str, name: &str) -> String {
    format!("{group}/channels/{name}")
}

fn channel(spec: &ChannelSpec) -> SocialPlace {
    let id = channel_id(spec.group, spec.name);
    let mut object = json!({ "id": id, "name": spec.name, "type": "text" });
    match spec.channel {
        Channel::Chat => {}
        Channel::Articles => {
            object["permissions"] = json!({ "post:message": [], "post:article": ["admin"] });
        }
        Channel::Call => object["type"] = json!("call"),
    }
    SocialPlace {
        account: SAM.into(),
        id,
        kind: PlaceKind::Channel,
        group: Some(spec.group.into()),
        followed: spec.followed,
        last_activity: None,
        object,
        cursors: PlaceCursors::default(),
        read_marker: None,
        notices: None,
    }
}

fn conversation(with: &str) -> SocialPlace {
    let id = conversation_id(with);
    SocialPlace {
        account: SAM.into(),
        id: id.clone(),
        kind: PlaceKind::Conversation,
        group: None,
        followed: true,
        last_activity: None,
        object: json!({ "id": id, "participants": [SAM, with] }),
        cursors: PlaceCursors::default(),
        read_marker: None,
        notices: None,
    }
}

fn conversation_id(with: &str) -> String {
    format!("dm_{}", with.split('@').next().unwrap_or(with))
}

/// The place a [`MessageSpec`] names.
fn place_id(place: &str) -> String {
    match place.split_once('/') {
        Some((group, name)) => {
            let group = match group {
                "rust" => RUST,
                "spec" => SPEC,
                "synth" => SYNTH,
                _ => BOOKS,
            };
            channel_id(group, name)
        }
        None => place.to_owned(),
    }
}

fn message(spec: &MessageSpec, n: usize, now_ms: i64) -> SocialMessage {
    let id = format!("m{n}");
    let created = now_ms - spec.minutes_ago * 60_000;
    let mut message: Value = json!({
        "id": id,
        "author": spec.author,
        "type": spec.kind,
        "content": { "mime": "text/markdown", "text": spec.text },
    });
    if spec.kind == "article" {
        message["title"] = json!(spec.text);
    }
    SocialMessage {
        account: SAM.into(),
        place: place_id(spec.place),
        id,
        author: spec.author.into(),
        kind: spec.kind.into(),
        created,
        parent: None,
        deleted: false,
        mentions_me: spec.mentions_me,
        unread: spec.unread,
        message,
        reactions: vec![],
    }
}

fn place(group: &str, name: &str) -> PlaceRef {
    PlaceRef {
        account: SAM.into(),
        id: channel_id(group, name),
    }
}

fn whole(uri: &str) -> GroupRef {
    GroupRef {
        account: SAM.into(),
        uri: uri.into(),
    }
}

/// Sam's own views, new (the store gives each its id and place).
fn own_views() -> Vec<SocialView> {
    let view = |name: &str, places: Vec<PlaceRef>, groups: Vec<GroupRef>| SocialView {
        id: String::new(),
        order: 0,
        name: name.into(),
        places,
        groups,
        filters: SocialFilters::default(),
        read_as: ReadAs::Stream,
        count: SocialCount::Unread,
    };
    let rust = view("Rust Audio", vec![], vec![whole(RUST)]);
    let spec = view(
        "Spec work",
        vec![
            place(SPEC, "chat"),
            place(SPEC, "proposals"),
            place(RUST, "ofscp"),
        ],
        vec![],
    );
    let mut reads = view(
        "Long reads",
        vec![
            place(SYNTH, "blog"),
            place(SPEC, "proposals"),
            place(RUST, "announcements"),
        ],
        vec![],
    );
    reads.read_as = ReadAs::List;
    reads.filters.kinds = vec![ContentKind::Article];
    let books = view("Book club", vec![], vec![whole(BOOKS)]);
    vec![rust, spec, reads, books]
}
