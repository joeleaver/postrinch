//! Provider-to-client events (spec §7.1, §7.4, §7.5, §5.7, §7.7, §10.1).

use serde::{Deserialize, Serialize};

use crate::wire::{
    DmConversation, ErrorCode, Member, Message, Notification, Presence, Reaction, ReadMarker,
    Timestamp,
};
use crate::{Actor, DmId, LocalId};

use super::{Empty, ws_body};

ws_body! {
    /// An event a provider sends.
    pub enum Event {
        /// Sent on connect: sign this nonce.
        AuthChallenge(AuthChallenge) = "auth.challenge",
        /// The connection is signed in.
        Authenticated(Authenticated) = "authenticated",
        /// The answer to `subscribe`.
        Subscribed(Subscribed) = "subscribed",
        /// The answer to `unsubscribe`, or, unasked, access was lost.
        Unsubscribed(Unsubscribed) = "unsubscribed",
        /// A message was posted in a subscribed channel.
        MessageCreated(ChannelMessage) = "message.created",
        /// A message was edited in a subscribed channel.
        MessageUpdated(ChannelMessage) = "message.updated",
        /// A message was deleted in a subscribed channel.
        MessageDeleted(MessageDeleted) = "message.deleted",
        /// Someone reacted to a message.
        ReactionAdded(ReactionAdded) = "reaction.added",
        /// Someone took a reaction back.
        ReactionRemoved(ReactionRemoved) = "reaction.removed",
        /// Someone is typing in a channel.
        ChannelTyping(ChannelTyping) = "channel.typing",
        /// A private message arrived in your inbox, or was edited or
        /// deleted there.
        DmMessage(DmMessage) = "dm.message",
        /// One of your conversations changed, from a send on any of your
        /// devices.
        DmConversation(DmConversationEvent) = "dm.conversation",
        /// A reaction on a private message changed.
        DmReaction(DmReaction) = "dm.reaction",
        /// Someone is typing in a private conversation.
        DmTyping(DmTyping) = "dm.typing",
        /// The answer to `presence.subscribe`.
        PresenceSubscribed(PresenceUsersAck) = "presence.subscribed",
        /// The answer to `presence.unsubscribe`.
        PresenceUnsubscribed(PresenceUsersAck) = "presence.unsubscribed",
        /// A subscribed user's presence changed.
        PresenceUpdate(PresenceChanged) = "presence.update",
        /// A member's role or nickname changed.
        MemberUpdated(MemberUpdated) = "member.updated",
        /// A new entry in your notification feed.
        NotificationCreated(NotificationCreated) = "notification.created",
        /// Your read markers moved on another device.
        ReadUpdated(ReadUpdated) = "read.updated",
        /// Are you there? Answer `pong`.
        Ping(Empty) = "ping",
        /// The answer to your `ping`.
        Pong(Empty) = "pong",
        /// A command failed.
        Error(WsError) = "error",
    }
}

/// `auth.challenge` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthChallenge {
    /// The nonce to sign.
    pub nonce: String,
    /// When the challenge expires.
    pub expires_at: Timestamp,
}

/// `authenticated` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authenticated {
    /// Who the connection is signed in as.
    pub actor: Actor,
}

/// `subscribed` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribed {
    /// The channels now subscribed.
    pub channels: Vec<LocalId>,
    /// Channels whose gap since their `since` cursor was too long to
    /// replay: backfill them over REST. The spec recommends this field in
    /// prose (§7.1, "Resuming after a disconnect"); the schema does not
    /// list it yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<Vec<LocalId>>,
}

/// `unsubscribed` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unsubscribed {
    /// The channels no longer subscribed.
    pub channels: Vec<LocalId>,
    /// Why, when the provider ended it unasked: `access_revoked`. Final
    /// for those channels until access is restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `message.created` and `message.updated`: the full message (§7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessage {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// Its position in the timeline, in the same space as REST history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// The message.
    pub message: Box<Message>,
}

/// `message.deleted` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDeleted {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// Its position in the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// The message.
    pub message_id: LocalId,
    /// When it was deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Timestamp>,
}

/// `reaction.added` (§7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionAdded {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// The reaction.
    pub reaction: Reaction,
}

/// `reaction.removed` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionRemoved {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// The message.
    pub message_id: LocalId,
    /// Which reaction.
    pub key: String,
    /// Whose.
    pub author: Actor,
}

/// Whether someone started or stopped typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TypingState {
    /// Started.
    Start,
    /// Stopped.
    Stop,
}

/// `channel.typing` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelTyping {
    /// The channel.
    pub channel_id: LocalId,
    /// Who.
    pub user: Actor,
    /// Started or stopped.
    pub state: TypingState,
}

/// `dm.message` (§7.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmMessage {
    /// The conversation.
    pub dm_id: DmId,
    /// Its position in the conversation, for resuming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// The message.
    pub message: Box<Message>,
}

/// `dm.conversation` (§7.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DmConversationEvent {
    /// The conversation as it is now.
    pub conversation: Box<DmConversation>,
}

/// Whether a reaction was added or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReactionState {
    /// Added.
    Added,
    /// Removed.
    Removed,
}

/// `dm.reaction` (§7.4). An addition carries the reaction; a removal
/// names its key and author.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmReaction {
    /// The conversation.
    pub dm_id: DmId,
    /// The message.
    pub message_id: LocalId,
    /// Added or removed.
    pub state: ReactionState,
    /// The reaction, when added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<Reaction>,
    /// Which reaction, when removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Whose, when removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Actor>,
}

/// `dm.typing` (§7.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmTyping {
    /// The conversation.
    pub dm_id: DmId,
    /// Who.
    pub user: Actor,
    /// Started or stopped.
    pub state: TypingState,
}

/// `presence.subscribed` and `presence.unsubscribed` (§7.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceUsersAck {
    /// The users the change applied to.
    pub users: Vec<Actor>,
}

/// `presence.update` (§7.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresenceChanged {
    /// Whose presence.
    pub user: Actor,
    /// What it is now.
    pub presence: Presence,
}

/// `member.updated` (§5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberUpdated {
    /// The group.
    pub group_id: LocalId,
    /// The member as they are now.
    pub member: Member,
}

/// `notification.created` (§10.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationCreated {
    /// The new entry.
    pub notification: Notification,
}

/// `read.updated` (§7.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadUpdated {
    /// The markers that moved.
    pub markers: Vec<ReadMarker>,
}

/// `error`: a command failed (§7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WsError {
    /// The `errorCode`.
    pub code: ErrorCode,
    /// What went wrong, for people.
    pub message: String,
    /// The matching HTTP status.
    pub status: u16,
    /// More about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
