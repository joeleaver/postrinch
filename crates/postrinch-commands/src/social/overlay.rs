//! The outbox laid over what sync kept (docs/ofscp.md "Storage"): a post
//! being sent shows as a message, an edit, a delete or a reaction shows
//! made, and a follow or a notices setting shows set, until the provider
//! has taken the change and sync brings back its copy.

use std::collections::HashSet;

use postrinch_store::{
    DraftMedia, ReactionSum, SocialChange, SocialContent, SocialGroup, SocialMessage,
    SocialOutboxEntry, SocialPlace, SocialPost,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{str_at, type_name};

/// A change of the person's still on its way to its provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialPending {
    /// The change's client id: a post's id until its provider has it.
    pub client_id: String,
    /// How many times sending it failed.
    pub attempts: u32,
    /// Why the last attempt failed; `None` while none has.
    pub error: Option<String>,
    /// Whether its provider refused it for good: it is not tried again,
    /// and waits to be given up.
    pub refused: bool,
    /// What a pending post or edit says, its pictures and files still to
    /// upload among them: the message shows what has been uploaded.
    pub content: Option<SocialContent>,
}

impl SocialPending {
    pub(crate) fn of(entry: &SocialOutboxEntry, content: Option<SocialContent>) -> Self {
        Self {
            client_id: entry.client_id.clone(),
            attempts: entry.attempts,
            error: entry.error.clone(),
            refused: entry.refused,
            content,
        }
    }
}

/// A message as shown: as sync kept it, with the person's pending changes
/// laid over it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialItem {
    pub message: SocialMessage,
    /// Whether its provider has it. A post still being sent has its client
    /// id for its id, and can't be edited, answered or reacted to yet.
    pub posted: bool,
    /// How many messages answer it, those being sent among them.
    pub replies: usize,
    /// The last of its changes still on its way, if any.
    pub pending: Option<SocialPending>,
}

/// The changes to one place still on their way, in the order made.
pub(crate) struct PlaceOutbox {
    entries: Vec<SocialOutboxEntry>,
}

impl PlaceOutbox {
    pub fn new(entries: Vec<SocialOutboxEntry>) -> Self {
        Self { entries }
    }

    /// The queued post whose client id is `id`.
    pub fn queued_post(&self, id: &str) -> Option<(&SocialOutboxEntry, &SocialPost)> {
        self.entries.iter().find_map(|e| match &e.change {
            SocialChange::Post(post) if e.client_id == id => Some((e, post)),
            _ => None,
        })
    }

    /// Whether an edit of `message` is being sent.
    pub fn edits(&self, message: &str) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(&e.change, SocialChange::Edit { message: id, .. } if id == message))
    }

    /// `message` with its pending edits, delete and reactions made.
    pub fn lay_over(&self, mut message: SocialMessage) -> SocialItem {
        let mut pending = None;
        for entry in &self.entries {
            match &entry.change {
                SocialChange::Edit {
                    message: id,
                    content,
                } if *id == message.id => {
                    edit(&mut message.message, content, entry.created);
                    pending = Some(SocialPending::of(entry, Some(content.clone())));
                }
                SocialChange::Delete { message: id } if *id == message.id => {
                    tombstone(&mut message, entry.created);
                    pending = Some(SocialPending::of(entry, None));
                }
                SocialChange::React {
                    message: id,
                    key,
                    on,
                } if *id == message.id => {
                    react(&mut message.reactions, key, *on);
                    pending = Some(SocialPending::of(entry, None));
                }
                _ => {}
            }
        }
        SocialItem {
            message,
            posted: true,
            replies: 0,
            pending,
        }
    }

    /// The posts being sent that answer `parent` (or none, for `None`), as
    /// messages, oldest first. Those whose copy sync already kept (named in
    /// `kept` by `clientMessageId`) are left out.
    pub fn queued(
        &self,
        account: &str,
        place: &str,
        parent: Option<&str>,
        kept: &HashSet<String>,
    ) -> Vec<SocialItem> {
        self.entries
            .iter()
            .filter_map(|entry| match &entry.change {
                SocialChange::Post(post)
                    if post.reply_to.as_deref() == parent && !kept.contains(&entry.client_id) =>
                {
                    Some(queued_item(entry, post, account, place))
                }
                _ => None,
            })
            .collect()
    }

    /// The client ids of the posts being sent that answer `parent`.
    pub fn queued_replies(&self, parent: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| {
                matches!(&e.change, SocialChange::Post(p) if p.reply_to.as_deref() == Some(parent))
            })
            .map(|e| e.client_id.as_str())
            .collect()
    }

    /// `place` with its pending follow and notices setting made. Returns
    /// the last such change still on its way.
    pub fn lay_over_place(&self, place: &mut SocialPlace) -> Option<SocialPending> {
        let mut pending = None;
        for entry in &self.entries {
            match &entry.change {
                SocialChange::Follow { on, .. } => place.followed = *on,
                SocialChange::ChannelNotices { notices } => place.notices = *notices,
                _ => continue,
            }
            pending = Some(SocialPending::of(entry, None));
        }
        pending
    }
}

/// `group` with the account's pending notices settings for it made, from
/// `outbox`, the account's changes. Returns the last one still on its way.
pub(crate) fn lay_over_group(
    group: &mut SocialGroup,
    outbox: &[SocialOutboxEntry],
) -> Option<SocialPending> {
    let mut pending = None;
    for entry in outbox {
        if let SocialChange::GroupNotices {
            group: uri,
            notices,
        } = &entry.change
            && *uri == group.uri
        {
            group.notices = *notices;
            pending = Some(SocialPending::of(entry, None));
        }
    }
    pending
}

/// The ids by which kept messages name the posts they were sent as.
pub(crate) fn client_ids<'a>(
    messages: impl IntoIterator<Item = &'a SocialMessage>,
) -> HashSet<String> {
    messages
        .into_iter()
        .filter_map(|m| str_at(&m.message, "clientMessageId").map(str::to_owned))
        .collect()
}

/// A post being sent, as a message (§5.3's Message, as far as it can be
/// before its provider has it).
fn queued_item(
    entry: &SocialOutboxEntry,
    post: &SocialPost,
    account: &str,
    place: &str,
) -> SocialItem {
    let mut object = json!({
        "id": entry.client_id,
        "clientMessageId": entry.client_id,
        "author": account,
        "type": type_name(post.kind),
        "createdAt": rfc3339(entry.created),
    });
    if let Some(parent) = &post.reply_to {
        object["reference"] = json!({ "type": "reply", "id": parent });
    }
    set_content(&mut object, &post.content);
    SocialItem {
        message: SocialMessage {
            account: account.to_owned(),
            place: place.to_owned(),
            id: entry.client_id.clone(),
            author: account.to_owned(),
            kind: type_name(post.kind).to_owned(),
            created: entry.created,
            parent: post.reply_to.clone(),
            deleted: false,
            mentions_me: false,
            unread: false,
            message: object,
            reactions: Vec::new(),
        },
        posted: false,
        replies: 0,
        pending: Some(SocialPending::of(entry, Some(post.content.clone()))),
    }
}

/// A message's object with `content` in place of what it said.
fn edit(object: &mut Value, content: &SocialContent, at: i64) {
    set_content(object, content);
    object["editedAt"] = json!(rfc3339(at));
}

/// Write `content` into a message's object: its body, title, tags, cover
/// and the attachments already uploaded.
fn set_content(object: &mut Value, content: &SocialContent) {
    object["content"] = json!({ "mime": "text/markdown", "text": content.body });
    object["tags"] = json!(content.tags);
    object["attachments"] = Value::Array(content.attachments.iter().filter_map(uploaded).collect());
    match &content.title {
        Some(title) => object["title"] = json!(title),
        None => remove(object, "title"),
    }
    match content.cover.as_ref().and_then(uploaded) {
        Some(cover) => object["cover"] = cover["id"].clone(),
        None => remove(object, "cover"),
    }
}

/// Take `key` out of an object.
fn remove(object: &mut Value, key: &str) {
    if let Some(fields) = object.as_object_mut() {
        fields.remove(key);
    }
}

/// The Attachment object of media already uploaded.
fn uploaded(media: &DraftMedia) -> Option<Value> {
    match media {
        DraftMedia::Uploaded { attachment } => Some(attachment.clone()),
        DraftMedia::File { .. } | DraftMedia::Kept { .. } => None,
    }
}

/// A message deleted, as its provider leaves it: a tombstone, its content
/// and attachments gone (§7.1).
fn tombstone(message: &mut SocialMessage, at: i64) {
    message.deleted = true;
    let object = &mut message.message;
    object["content"] = json!({ "mime": "text/plain", "text": "" });
    object["attachments"] = json!([]);
    object["deletedAt"] = json!(rfc3339(at));
}

/// The account's reaction `key` added (`on`) or taken back.
fn react(reactions: &mut Vec<ReactionSum>, key: &str, on: bool) {
    let at = reactions.iter().position(|r| r.key == key);
    match (at, on) {
        (Some(i), true) if !reactions[i].mine => {
            reactions[i].mine = true;
            reactions[i].count += 1;
        }
        (None, true) => reactions.push(ReactionSum {
            key: key.to_owned(),
            count: 1,
            mine: true,
        }),
        (Some(i), false) if reactions[i].mine => {
            reactions[i].mine = false;
            reactions[i].count = reactions[i].count.saturating_sub(1);
            if reactions[i].count == 0 {
                reactions.remove(i);
            }
        }
        _ => {}
    }
}

/// A time in milliseconds as RFC 3339, as the protocol writes times.
fn rfc3339(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis)
        .map(|t| t.to_string())
        .unwrap_or_default()
}
