//! The records the store keeps for social (OFSCP) accounts. Protocol
//! objects are held whole as JSON values: their types belong to
//! `postrinch-ofscp`, and the store keeps whatever a provider sent, fields
//! it does not know included. Times are milliseconds since the epoch.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::SocialChange;

/// A social account: an actor on its home provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialAccount {
    /// The actor, `handle@domain`, as the person wrote it. The store keys
    /// it lower-cased ([`super::social_account_key`]).
    pub actor: String,
    pub state: SocialAccountState,
    /// The account's UserProfile (§5.1.1) as the home provider last gave
    /// it; `None` before the first sync.
    pub profile: Option<Value>,
    /// The account's own settings as its home provider keeps them (§10.2:
    /// notification settings); `None` before the first sync.
    pub settings: Option<Value>,
    /// The id the home provider gave this computer's device key. The key
    /// itself is in the OS keychain; `None` while signed out.
    pub key_id: Option<String>,
}

/// Whether a social account can talk to its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocialAccountState {
    Active,
    /// Its device key was refused (revoked, WebSocket close 4003, or a 401
    /// on a fresh signature): signing in again makes a new one.
    NeedsSignIn,
    /// Its provider's clock is further from this one's than signatures
    /// allow, even with the offset learned from its `Date` header.
    ClockSkewed,
}

impl SocialAccountState {
    /// The name the store indexes the state by.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NeedsSignIn => "needs_sign_in",
            Self::ClockSkewed => "clock_skewed",
        }
    }
}

/// What Postrinch knows of a provider, shared by every account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialProvider {
    /// Its domain, as discovery found it. The store keys it lower-cased.
    pub domain: String,
    /// Its discovery document (`/.well-known/ofscp`), whole.
    pub discovery: Value,
    /// Its capabilities document, whole.
    pub capabilities: Value,
    /// How far its clock is ahead of this one's, from its `Date` header,
    /// in milliseconds (negative when behind). Signatures are made with it.
    pub clock_offset_ms: i64,
}

/// How a place or group tells the person of new messages: the spec's
/// notification settings (§10.2), synced with the home provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notices {
    Everything,
    MentionsAndReplies,
    Nothing,
}

/// How Postrinch came to know a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupFound {
    /// The account is a member (its home provider lists the group).
    Member,
    /// From an invite link the person opened.
    Invite,
    /// From a provider's list of groups, to browse in All channels.
    Directory,
}

/// A group, as one account knows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialGroup {
    /// The account's actor, as [`SocialAccount::actor`].
    pub account: String,
    /// The group's URI.
    pub uri: String,
    /// The domain of the provider that holds it.
    pub provider: String,
    /// The Group object, whole.
    pub group: Value,
    /// The account's role in it, as the provider names it; `None` when not
    /// a member.
    pub role: Option<String>,
    pub found: GroupFound,
    /// The group's notification setting; `None` for the provider's default.
    pub notices: Option<Notices>,
}

/// What a place is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    /// A group's channel, keyed by its URI.
    Channel,
    /// A private conversation, keyed by its dmId.
    Conversation,
}

impl PlaceKind {
    /// The name the store indexes the kind by.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Conversation => "conversation",
        }
    }
}

/// Where a place's history got to. Cursors are opaque and valid only on
/// the place that gave them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceCursors {
    /// The newest event's cursor: live events resume from it.
    pub live: Option<String>,
    /// Where the next older page starts, for scrolling back.
    pub older: Option<String>,
    /// Whether every older page is here.
    pub complete: bool,
    /// A conversation with someone on another provider: the newest
    /// event's cursor there, where the messages the person sent are held
    /// (§7.4). Live events from a `dmId` subscription there resume from
    /// it. `None` for a channel, and for a conversation within one
    /// provider.
    #[serde(default)]
    pub sent: Option<String>,
}

/// A channel or a private conversation, as one account knows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialPlace {
    /// The account's actor.
    pub account: String,
    /// The channel's URI, or the conversation's dmId.
    pub id: String,
    pub kind: PlaceKind,
    /// The channel's group URI; `None` for a conversation.
    pub group: Option<String>,
    pub followed: bool,
    /// When its newest message was made; `None` before any.
    pub last_activity: Option<i64>,
    /// The Channel or DmConversation object, whole.
    pub object: Value,
    pub cursors: PlaceCursors,
    /// The id of the newest message read here, as the read marker has it.
    pub read_marker: Option<String>,
    /// A channel's notification setting; `None` for its group's (and
    /// always for a conversation, which always notifies unless muted).
    pub notices: Option<Notices>,
    /// A private conversation muted on this computer: it tells of no new
    /// message. Local, never sent to a provider (§10.2 covers channels
    /// and groups only). Always `false` for a channel, which has
    /// [`Self::notices`].
    #[serde(default)]
    pub muted: bool,
}

/// A place named across accounts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaceRef {
    pub account: String,
    pub id: String,
}

/// A group named across accounts: in a view, all its channels, including
/// ones made later.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GroupRef {
    pub account: String,
    pub uri: String,
}

/// What kind of thing a message or draft is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Chat,
    Memo,
    Article,
}

/// How a view shows its messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadAs {
    /// A stream, as chat reads.
    #[default]
    Stream,
    /// A list with a reader, as mail reads.
    List,
}

/// What the sidebar counts beside a social view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocialCount {
    /// Unread messages.
    #[default]
    Unread,
    /// Unread messages that mention the person.
    Mentions,
    /// Nothing.
    None,
}

/// A view's filters over the messages of its places.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialFilters {
    /// The kinds it shows; empty for every kind.
    pub kinds: Vec<ContentKind>,
    /// Words, `from:` and `tag:`, as the person wrote them; empty for none.
    pub search: String,
}

/// A view the person made: a selection of places and groups.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialView {
    pub id: String,
    /// Where it sits among the views, smallest first.
    pub order: u64,
    pub name: String,
    pub places: Vec<PlaceRef>,
    pub groups: Vec<GroupRef>,
    pub filters: SocialFilters,
    pub read_as: ReadAs,
    pub count: SocialCount,
}

/// One reaction on a message, summed over who gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionSum {
    /// The reaction's key (an emoji, or a custom reaction's name).
    pub key: String,
    pub count: u32,
    /// Whether the account is one who gave it.
    pub mine: bool,
}

/// A message in a place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialMessage {
    /// The account's actor.
    pub account: String,
    /// The place's id ([`SocialPlace::id`]).
    pub place: String,
    /// The message's id.
    pub id: String,
    /// Its author's actor.
    pub author: String,
    /// Its type as the provider gives it (`message`, `memo`, `article`,
    /// or one this version does not know).
    pub kind: String,
    pub created: i64,
    /// The id of the message it answers, in the same place.
    pub parent: Option<String>,
    pub deleted: bool,
    pub mentions_me: bool,
    pub unread: bool,
    /// The Message object, whole.
    pub message: Value,
    pub reactions: Vec<ReactionSum>,
}

/// A picture, video or file a message carries, or a user's avatar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialMedia {
    /// The account's actor.
    pub account: String,
    /// The domain of the provider that issued the attachment: its
    /// uploader's home (§5.8), so the author's; for an avatar, the user's.
    pub provider: String,
    /// The attachment's id on that provider; for a user's avatar, which
    /// has none, `@` and the user's handle (a local id never holds `@`).
    pub id: String,
    /// Where it is fetched from (unsigned: a capability URL).
    pub url: String,
    /// SHA-256 of its bytes in the media cache; `None` until fetched, and
    /// again once evicted.
    pub blob: Option<String>,
    /// When the bytes were fetched.
    pub fetched: Option<i64>,
    /// When the cached bytes were last asked for: the cache evicts the
    /// least recently used first. `None` until they are asked for again
    /// after being fetched.
    #[serde(default)]
    pub used: Option<i64>,
    pub mime: Option<String>,
    pub size: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// A video's or a sound's length, in milliseconds.
    pub duration_ms: Option<u64>,
    /// A video's poster picture, by URL.
    pub poster: Option<String>,
}

/// A picture, video or file on a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "from")]
pub enum DraftMedia {
    /// A file on this computer, picked or dropped, not yet uploaded.
    File { path: PathBuf, mime: Option<String> },
    /// Bytes kept in the blob store (a pasted picture), not yet uploaded.
    Kept {
        blob: String,
        name: String,
        mime: Option<String>,
    },
    /// Uploaded: the Attachment object the provider gave back, whole.
    Uploaded { attachment: Value },
}

/// A post, reply or message being written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialDraft {
    pub id: String,
    /// The account's actor.
    pub account: String,
    /// The place it goes to; `None` until chosen.
    pub place: Option<String>,
    /// The message it answers, in that place.
    pub reply_to: Option<String>,
    pub updated: i64,
    pub kind: ContentKind,
    pub title: Option<String>,
    /// The body, in Markdown.
    pub body: String,
    pub tags: Vec<String>,
    pub cover: Option<DraftMedia>,
    pub attachments: Vec<DraftMedia>,
}

/// A notification from an account's home provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialNotification {
    /// The account's actor.
    pub account: String,
    pub id: String,
    pub created: i64,
    pub read: bool,
    /// The Notification object, whole.
    pub notification: Value,
}

/// A change made here, waiting to reach a provider: a post, reply, edit,
/// delete, reaction, read marker, follow, conversation pointer or profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialOutboxEntry {
    /// The account's actor.
    pub account: String,
    /// The change's client id, sent with it so a retry is not made twice.
    pub client_id: String,
    /// The place's id; `None` for a change of no place (the profile).
    pub place: Option<String>,
    /// The order changes were made in, across every account.
    pub seq: u64,
    /// The change, as the engine will send it.
    pub change: SocialChange,
    pub created: i64,
    /// Whether the engine has begun sending it: from then on it may reach
    /// its provider, so it can no longer be taken back.
    pub tried: bool,
    /// How many attempts failed.
    pub attempts: u32,
    /// Why the last attempt failed; `None` while none has.
    pub error: Option<String>,
    /// Whether its provider refused it for good (a `403`, a `400`):
    /// it is not tried again, and waits for the person to give it up.
    #[serde(default)]
    pub refused: bool,
    /// The files of its change uploaded so far, each with the Attachment
    /// its provider gave back: sent in their place. The change keeps the
    /// files as picked, so one whose upload grew too old to post (§5.8: a
    /// provider keeps an upload nothing posted for at least a day) is
    /// uploaded again.
    #[serde(default)]
    pub uploads: Vec<UploadedFile>,
}

/// A file of a queued change, uploaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UploadedFile {
    /// The file as the change carries it.
    pub media: DraftMedia,
    /// The Attachment its provider gave back, whole.
    pub attachment: Value,
    /// When it was uploaded.
    pub at: i64,
}
