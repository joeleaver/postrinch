//! Client-to-provider commands (spec §7.1, §7.5).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::wire::{Content, MessageCreate, PresenceUpdate};
use crate::{Actor, LocalId};

use super::{Empty, ws_body};

ws_body! {
    /// A command a client sends.
    pub enum Command {
        /// Sign the connection in. Must be the first command.
        Authenticate(Authenticate) = "authenticate",
        /// Start receiving channels' events.
        Subscribe(Subscribe) = "subscribe",
        /// Stop receiving channels' events.
        Unsubscribe(Unsubscribe) = "unsubscribe",
        /// Post a message, memo or article.
        MessageCreate(ChannelMessageCreate) = "message.create",
        /// Edit your message.
        MessageUpdate(ChannelMessageUpdate) = "message.update",
        /// Delete a message (it becomes a tombstone).
        MessageDelete(ChannelMessageDelete) = "message.delete",
        /// React to a message.
        ReactionAdd(ReactionAdd) = "reaction.add",
        /// Take your reaction back.
        ReactionRemove(ReactionRemove) = "reaction.remove",
        /// You started typing in a channel.
        TypingStart(Typing) = "typing.start",
        /// You stopped typing in a channel.
        TypingStop(Typing) = "typing.stop",
        /// Start receiving users' presence.
        PresenceSubscribe(PresenceUsers) = "presence.subscribe",
        /// Stop receiving users' presence.
        PresenceUnsubscribe(PresenceUsers) = "presence.unsubscribe",
        /// Set your own presence.
        PresenceSet(PresenceUpdate) = "presence.set",
        /// Are you there? The provider answers `pong`.
        Ping(Empty) = "ping",
        /// The answer to the provider's `ping`, with its `id` as
        /// `correlationId`.
        Pong(Empty) = "pong",
    }
}

/// `authenticate` (§7.1). The signature covers the provider's authority,
/// the challenge nonce and `timestamp`, from
/// [`crate::sign::Signer::sign_ws_authenticate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Authenticate {
    /// Who is signing in.
    pub actor: Actor,
    /// The device key's id.
    pub key_id: String,
    /// The exact timestamp that was signed (RFC 3339, UTC).
    pub timestamp: String,
    /// The signature, base64.
    pub signature: String,
}

/// `subscribe` (§7.1). To resume, give each channel's last cursor in
/// `since`; the provider replays what was missed, or says it was truncated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribe {
    /// The channels, by local id on this provider.
    pub channels: Vec<LocalId>,
    /// Which event types to receive (the provider decides when left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    /// The last cursor seen, per channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<BTreeMap<LocalId, String>>,
}

/// `unsubscribe` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unsubscribe {
    /// The channels.
    pub channels: Vec<LocalId>,
}

/// `message.create`: a new message in a channel (§7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessageCreate {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// The message.
    #[serde(flatten)]
    pub message: MessageCreate,
}

/// `message.update`: an edit of your message (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessageUpdate {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// The message.
    pub message_id: LocalId,
    /// Its new content.
    pub content: Content,
}

/// `message.delete` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessageDelete {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// The message.
    pub message_id: LocalId,
}

/// `reaction.add` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionAdd {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// The message.
    pub message_id: LocalId,
    /// Which reaction, such as `heart`.
    pub key: String,
    /// Its emoji.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unicode: Option<String>,
    /// A picture of it (HTTPS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// `reaction.remove` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionRemove {
    /// The channel's group.
    pub group_id: LocalId,
    /// The channel.
    pub channel_id: LocalId,
    /// The message.
    pub message_id: LocalId,
    /// Which reaction.
    pub key: String,
}

/// `typing.start` and `typing.stop` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Typing {
    /// The channel.
    pub channel_id: LocalId,
}

/// `presence.subscribe` and `presence.unsubscribe` (§7.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceUsers {
    /// At least one user.
    pub users: Vec<Actor>,
}
