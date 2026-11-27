//! Tests of the social commands, with the world they share: Ada's account
//! on groups.example, a member of a group there and of one on far.example,
//! with a private conversation; and Carl's account on other.example, for
//! what an agent limited to one account sees.

mod directory;
mod notices;
mod pages;
mod places;
mod posting;
mod scope;
mod search;
mod views;

use postrinch_store::{
    ContentKind, GroupFound, Notices, PlaceCursors, PlaceKind, SocialAccount, SocialAccountState,
    SocialDraft, SocialGroup, SocialMessage, SocialPlace, Store,
};
use serde_json::{Value, json};

use crate::Commands;

const ADA: &str = "ada@groups.example";
const CARL: &str = "carl@other.example";
/// Who writes to Ada's places.
const BOB: &str = "bob@far.example";

/// Ada's home group, and its channel: a home channel, with a read marker
/// at the provider (§7.7).
const GROUP: &str = "https://groups.example/api/groups/g1";
const GENERAL: &str = "https://groups.example/api/groups/g1/channels/general";
/// A group on another provider, and its channel, which Ada does not follow.
const FAR: &str = "https://far.example/api/groups/g2";
const NEWS: &str = "https://far.example/api/groups/g2/channels/news";
/// Ada's private conversation with Bob.
const DM: &str = "dm_bob";
/// Carl's group and channel.
const CARL_GROUP: &str = "https://other.example/api/groups/g3";
const CARL_CHANNEL: &str = "https://other.example/api/groups/g3/channels/lobby";

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    commands: Commands,
}

impl World {
    /// Keep `message`, searchable by its body.
    fn put(&self, message: &SocialMessage) {
        let text = body_of(message);
        self.store.put_social_message(message, &text).unwrap();
    }

    /// Keep a message from Bob that answers none.
    fn bob(&self, place: &str, id: &str, created: i64, text: &str) -> SocialMessage {
        let message = message(ADA, place, id, created, BOB, text);
        self.put(&message);
        message
    }

    /// Give a place's `permissions` (§5.2.1).
    fn channel_permissions(&self, place: &str, permissions: Value) {
        self.store
            .change_social_place(ADA, place, |p| {
                p.object["permissions"] = permissions.clone();
            })
            .unwrap();
    }

    /// Give a group's `permissions` and Ada's role in it.
    fn group_permissions(&self, uri: &str, role: Option<&str>, permissions: Value) {
        let mut group = self.store.social_group(ADA, uri).unwrap().unwrap();
        group.group["permissions"] = permissions;
        group.role = role.map(str::to_owned);
        self.store.put_social_group(&group).unwrap();
    }

    /// A draft of Ada's with `body`, ready to send.
    fn draft(
        &self,
        place: &str,
        reply_to: Option<&str>,
        kind: ContentKind,
        body: &str,
    ) -> SocialDraft {
        let mut draft = self
            .commands
            .start_social_draft(ADA, Some(place), reply_to, kind)
            .unwrap();
        draft.body = body.into();
        draft
    }
}

/// The world the tests share.
fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.put_social_account(&account(ADA, "Ada")).unwrap();
    store.put_social_account(&account(CARL, "Carl")).unwrap();
    store
        .put_social_group(&group(ADA, GROUP, "Home group"))
        .unwrap();
    store
        .put_social_group(&group(ADA, FAR, "Far group"))
        .unwrap();
    store
        .put_social_group(&group(CARL, CARL_GROUP, "Carl's group"))
        .unwrap();
    store
        .put_social_place(&channel(ADA, GENERAL, GROUP, "general", true))
        .unwrap();
    store
        .put_social_place(&channel(ADA, NEWS, FAR, "news", false))
        .unwrap();
    store.put_social_place(&conversation(ADA, DM)).unwrap();
    store
        .put_social_place(&channel(CARL, CARL_CHANNEL, CARL_GROUP, "lobby", true))
        .unwrap();
    World {
        _dir: dir,
        commands: Commands::new(store.clone()),
        store,
    }
}

fn account(actor: &str, name: &str) -> SocialAccount {
    SocialAccount {
        actor: actor.into(),
        state: SocialAccountState::Active,
        profile: Some(json!({ "handle": actor.split('@').next(), "displayName": name })),
        settings: None,
        key_id: Some("dk_1".into()),
    }
}

fn group(account: &str, uri: &str, name: &str) -> SocialGroup {
    SocialGroup {
        account: account.into(),
        uri: uri.into(),
        provider: uri
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap()
            .into(),
        group: json!({ "id": uri, "name": name }),
        role: Some("member".into()),
        found: GroupFound::Member,
        notices: None,
    }
}

fn channel(account: &str, id: &str, group: &str, name: &str, followed: bool) -> SocialPlace {
    SocialPlace {
        account: account.into(),
        id: id.into(),
        kind: PlaceKind::Channel,
        group: Some(group.into()),
        followed,
        last_activity: None,
        object: json!({ "id": id, "name": name }),
        cursors: PlaceCursors::default(),
        read_marker: None,
        notices: Some(Notices::MentionsAndReplies),
        muted: false,
    }
}

fn conversation(account: &str, id: &str) -> SocialPlace {
    SocialPlace {
        account: account.into(),
        id: id.into(),
        kind: PlaceKind::Conversation,
        group: None,
        followed: true,
        last_activity: None,
        object: json!({ "id": id, "participants": [account, BOB] }),
        cursors: PlaceCursors::default(),
        read_marker: None,
        notices: None,
        muted: false,
    }
}

/// A chat message, unread, as sync keeps one.
fn message(
    account: &str,
    place: &str,
    id: &str,
    created: i64,
    author: &str,
    text: &str,
) -> SocialMessage {
    SocialMessage {
        account: account.into(),
        place: place.into(),
        id: id.into(),
        author: author.into(),
        kind: "message".into(),
        created,
        parent: None,
        deleted: false,
        mentions_me: false,
        unread: true,
        message: json!({
            "id": id,
            "author": author,
            "type": "message",
            "content": { "mime": "text/markdown", "text": text },
        }),
        reactions: vec![],
    }
}

fn body_of(message: &SocialMessage) -> String {
    message
        .message
        .pointer("/content/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The ids of what a page or a search shows, in order.
fn ids(items: &[crate::SocialItem]) -> Vec<&str> {
    items.iter().map(|i| i.message.id.as_str()).collect()
}

/// The message an error says, for asserting on.
fn refused<T: std::fmt::Debug>(result: crate::CommandResult<T>) -> String {
    match result {
        Ok(value) => panic!("expected a refusal, got {value:?}"),
        Err(error) => error.to_string(),
    }
}
