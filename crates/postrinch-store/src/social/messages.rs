//! The messages of places. A message's scalars are a `SocialMessage`
//! object, small because rhypedb copies it into each of its indexes; what
//! it says (the protocol message, its reactions, its search text) is its
//! `SocialMessageContent` (see `schema.rhype`).

use rhypedb_engine::CompareOp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{KEY, RECORD, ReactionSum, SocialMessage, scoped, social_account_key};
use crate::codec::{Fields, FieldsBuilder};
use crate::keys::SEP;
use crate::{Store, StoreError, StoreResult};

pub(crate) const MESSAGE: &str = "SocialMessage";
pub(crate) const CONTENT: &str = "SocialMessageContent";

/// What a message's content keeps in `record`.
#[derive(Serialize, Deserialize, PartialEq)]
struct Body {
    message: Value,
    reactions: Vec<ReactionSum>,
}

/// What keeping a message did ([`Store::put_social_message`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MessagePut {
    /// Whether it changed what is kept: a replayed event changes nothing.
    pub changed: bool,
    /// The client id of the account's queued post it took off the outbox,
    /// being that post's copy: the post is sent.
    pub settled: Option<String>,
}

fn message_key(account: &str, place: &str, id: &str) -> String {
    format!("{}{SEP}{id}", scoped(account, place))
}

impl Store {
    /// Keep a message as its place has it, with the text full-text search
    /// finds it by (its plain text and its author's name). Returns whether
    /// that changed anything: a replayed event is not written again. A
    /// repeat of the same call finishes what an interrupted one left.
    ///
    /// A message of the account's own that carries the client id of a post
    /// queued to its place (`clientMessageId`) takes that post off the
    /// outbox, and says so ([`MessagePut::settled`]), whether or not the
    /// message itself changed: this is the one place a post leaves the
    /// outbox for being sent, however its copy arrived.
    ///
    /// [`SocialMessage::unread`] is taken only for a message not here yet,
    /// and never for the person's own:
    /// whether a kept one is read is the read marker's and the person's
    /// ([`Store::set_social_messages_read`],
    /// [`Store::set_social_message_unread`]), so an edit or a reaction
    /// arriving never makes a read message unread again.
    pub fn put_social_message(
        &self,
        message: &SocialMessage,
        search_text: &str,
    ) -> StoreResult<MessagePut> {
        self.social_locked(|| {
            // The message first: readers do not take the lock, so in
            // between one sees the post both queued and kept rather than
            // neither. Interrupted there, the post is sent again, and the
            // provider answers with this copy (§7.1, `clientMessageId`).
            let changed = self.write_social_message(message, search_text)?;
            let settled = self.settle_own_post(message)?;
            Ok(MessagePut { changed, settled })
        })
    }

    /// Take off the outbox the account's queued post `message` is the copy
    /// of, if it is one, returning its client id. The client id is matched
    /// only on the account's own messages: others choose theirs, and may
    /// choose the same.
    fn settle_own_post(&self, message: &SocialMessage) -> StoreResult<Option<String>> {
        let account = social_account_key(&message.account);
        if social_account_key(&message.author) != account {
            return Ok(None);
        }
        let Some(client_id) = message
            .message
            .get("clientMessageId")
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };
        let settled = self.settle_social_post(&account, &message.place, client_id)?;
        Ok(settled.then(|| client_id.to_owned()))
    }

    fn write_social_message(
        &self,
        message: &SocialMessage,
        search_text: &str,
    ) -> StoreResult<bool> {
        let key = message_key(&message.account, &message.place, &message.id);
        let kept = self.social_message_at(&key)?;
        let account = social_account_key(&message.account);
        // As it reads back: by its account's key, read as it is here. What
        // the person wrote is never unread to them.
        let own = social_account_key(&message.author) == account;
        let message = &SocialMessage {
            unread: kept
                .as_ref()
                .map_or(message.unread && !own, |kept| kept.unread),
            account,
            ..message.clone()
        };
        if kept.as_ref() == Some(message)
            && self.social_search_text(&key)?.as_deref() == Some(search_text)
        {
            return Ok(false);
        }
        let parent = message
            .parent
            .as_ref()
            .map(|parent| message_key(&message.account, &message.place, parent));
        let scalars = FieldsBuilder::new()
            .str("place", scoped(&message.account, &message.place))
            .str("author", message.author.clone())
            .str("kind", message.kind.clone())
            .datetime("created", message.created)
            .opt_str("parent", parent)
            .bool("deleted", message.deleted)
            .bool("mentions_me", message.mentions_me)
            .bool("unread", message.unread);
        let id = self.social_put(MESSAGE, KEY, &key, scalars)?;
        let body = Body {
            message: message.message.clone(),
            reactions: message.reactions.clone(),
        };
        let content = FieldsBuilder::new()
            .str("search_text", search_text)
            .json(RECORD, &body);
        let create = content
            .clone()
            .str(KEY, key.clone())
            .relation("message", id);
        self.upsert(CONTENT, KEY, &key, create.build(), content.build())?;
        Ok(true)
    }

    /// The message `id` of a place.
    pub fn social_message(
        &self,
        account: &str,
        place: &str,
        id: &str,
    ) -> StoreResult<Option<SocialMessage>> {
        self.social_message_at(&message_key(account, place, id))
    }

    fn social_message_at(&self, key: &str) -> StoreResult<Option<SocialMessage>> {
        match self.find(MESSAGE, KEY, key)? {
            Some(scalars) => self.decode_social_message(&scalars),
            None => Ok(None),
        }
    }

    fn social_search_text(&self, key: &str) -> StoreResult<Option<String>> {
        match self.find(CONTENT, KEY, key)? {
            Some(content) => content.opt_str("search_text"),
            None => Ok(None),
        }
    }

    /// Up to `limit` of a place's messages that answer none, newest first,
    /// from just before `before` (the `created` and id of the oldest one
    /// shown) or from the newest.
    pub fn social_messages(
        &self,
        account: &str,
        place: &str,
        before: Option<(i64, &str)>,
        limit: usize,
    ) -> StoreResult<Vec<SocialMessage>> {
        let before = before.map(|(created, id)| (created, message_key(account, place, id)));
        let mut found = Vec::new();
        for scalars in self.social_where(MESSAGE, "place", &scoped(account, place))? {
            if scalars.opt_str("parent")?.is_some() {
                continue;
            }
            let at = (scalars.datetime("created")?, scalars.str(KEY)?);
            if before.as_ref().is_none_or(|before| &at < before) {
                found.push((at, scalars));
            }
        }
        found.sort_by(|a, b| b.0.cmp(&a.0));
        self.decode_social_messages(found.into_iter().map(|(_, s)| s), limit)
    }

    /// The messages that answer `parent` in a place, oldest first.
    pub fn social_replies(
        &self,
        account: &str,
        place: &str,
        parent: &str,
    ) -> StoreResult<Vec<SocialMessage>> {
        let mut found = self.social_replies_scalars(account, place, parent)?;
        found.sort_by(|a, b| a.0.cmp(&b.0));
        self.decode_social_messages(found.into_iter().map(|(_, s)| s), usize::MAX)
    }

    /// How many messages answer `parent` in a place.
    pub fn social_reply_count(
        &self,
        account: &str,
        place: &str,
        parent: &str,
    ) -> StoreResult<usize> {
        Ok(self.social_replies_scalars(account, place, parent)?.len())
    }

    fn social_replies_scalars(
        &self,
        account: &str,
        place: &str,
        parent: &str,
    ) -> StoreResult<Vec<((i64, String), Fields)>> {
        let parent = message_key(account, place, parent);
        self.social_where(MESSAGE, "parent", &parent)?
            .into_iter()
            .map(|s| Ok(((s.datetime("created")?, s.str(KEY)?), s)))
            .collect()
    }

    /// How many of a place's messages are unread.
    pub fn social_unread_count(&self, account: &str, place: &str) -> StoreResult<usize> {
        let mut unread = 0;
        for scalars in self.social_where(MESSAGE, "place", &scoped(account, place))? {
            if scalars.bool("unread")? {
                unread += 1;
            }
        }
        Ok(unread)
    }

    /// A place's unread messages, replies among them, newest first.
    pub fn social_unread_messages(
        &self,
        account: &str,
        place: &str,
    ) -> StoreResult<Vec<SocialMessage>> {
        self.social_place_messages_where(account, place, true)
    }

    /// Every message of a place, replies among them, newest first.
    pub fn social_place_messages(
        &self,
        account: &str,
        place: &str,
    ) -> StoreResult<Vec<SocialMessage>> {
        self.social_place_messages_where(account, place, false)
    }

    fn social_place_messages_where(
        &self,
        account: &str,
        place: &str,
        unread_only: bool,
    ) -> StoreResult<Vec<SocialMessage>> {
        let mut found = Vec::new();
        for scalars in self.social_where(MESSAGE, "place", &scoped(account, place))? {
            if !unread_only || scalars.bool("unread")? {
                found.push(((scalars.datetime("created")?, scalars.str(KEY)?), scalars));
            }
        }
        found.sort_by(|a, b| b.0.cmp(&a.0));
        self.decode_social_messages(found.into_iter().map(|(_, s)| s), usize::MAX)
    }

    /// Up to `limit` messages of every account that mention the person,
    /// newest first.
    pub fn social_mentions(&self, limit: usize) -> StoreResult<Vec<SocialMessage>> {
        let found = self
            .db
            .filter_scan_bool(MESSAGE, "mentions_me", CompareOp::Eq, true, None)?;
        let mut found = found
            .into_iter()
            .map(|o| {
                let s = Fields::new(MESSAGE, o);
                Ok(((s.datetime("created")?, s.str(KEY)?), s))
            })
            .collect::<StoreResult<Vec<_>>>()?;
        found.sort_by(|a, b| b.0.cmp(&a.0));
        self.decode_social_messages(found.into_iter().map(|(_, s)| s), limit)
    }

    /// Mark read every message of a place made at or before `up_to`, as a
    /// read marker moving there does. Returns how many were unread.
    pub fn set_social_messages_read(
        &self,
        account: &str,
        place: &str,
        up_to: i64,
    ) -> StoreResult<usize> {
        let _held = self.social_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut marked = 0;
        for scalars in self.social_where(MESSAGE, "place", &scoped(account, place))? {
            if scalars.bool("unread")? && scalars.datetime("created")? <= up_to {
                let read = FieldsBuilder::new().bool("unread", false).build();
                self.db_update(MESSAGE, scalars.id(), read)?;
                marked += 1;
            }
        }
        Ok(marked)
    }

    /// Mark one message read or unread. Returns whether it is here.
    pub fn set_social_message_unread(
        &self,
        account: &str,
        place: &str,
        id: &str,
        unread: bool,
    ) -> StoreResult<bool> {
        let _held = self.social_lock.lock().unwrap_or_else(|e| e.into_inner());
        match self.find(MESSAGE, KEY, &message_key(account, place, id))? {
            Some(scalars) => {
                let fields = FieldsBuilder::new().bool("unread", unread).build();
                self.db_update(MESSAGE, scalars.id(), fields)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The best `limit` full-text matches for `text` among every account's
    /// messages, best first. `text` is in rhypedb's `.matches` language.
    pub fn search_social_messages(
        &self,
        text: &str,
        limit: usize,
    ) -> StoreResult<Vec<SocialMessage>> {
        let result = self
            .db
            .fulltext_search(CONTENT, "search_text", text, limit, None, None)?;
        let mut found = Vec::with_capacity(result.hits.len());
        for hit in &result.hits {
            if let Some(id) = self.link(CONTENT, hit.object_id, "message")?
                && let Some(message) = self.decode_social_message(&self.get(MESSAGE, id)?)?
            {
                found.push(message);
            }
        }
        Ok(found)
    }

    /// Forget a message for good (its content goes with it). A message its
    /// provider deleted is kept, marked [`SocialMessage::deleted`].
    pub fn remove_social_message(&self, account: &str, place: &str, id: &str) -> StoreResult<()> {
        self.social_remove(MESSAGE, KEY, &message_key(account, place, id))?;
        Ok(())
    }

    /// Forget every message of `account`, whether or not its place is
    /// here (a message may come before its place, or be written while the
    /// account is being removed). Returns how many went.
    pub(super) fn remove_social_messages_of(&self, account: &str) -> StoreResult<usize> {
        let prefix = format!("{}{SEP}", social_account_key(account));
        let mut ids = Vec::new();
        for object in self.db.scan_type_projected(MESSAGE, &[KEY])? {
            let scalars = Fields::new(MESSAGE, object);
            if scalars.str(KEY)?.starts_with(&prefix) {
                ids.push(scalars.id());
            }
        }
        for &id in &ids {
            self.db_delete(MESSAGE, id)?;
        }
        Ok(ids.len())
    }

    /// Forget every message of the place whose key is `place`.
    pub(super) fn remove_social_messages_of_place(&self, place: &str) -> StoreResult<usize> {
        self.social_remove_where(MESSAGE, "place", place)
    }

    /// The first `limit` of `scalars` that have their content, decoded.
    fn decode_social_messages(
        &self,
        scalars: impl Iterator<Item = Fields>,
        limit: usize,
    ) -> StoreResult<Vec<SocialMessage>> {
        let mut found = Vec::new();
        for scalars in scalars {
            if found.len() == limit {
                break;
            }
            if let Some(message) = self.decode_social_message(&scalars)? {
                found.push(message);
            }
        }
        Ok(found)
    }

    /// A message from its scalars and content. `None` when its content is
    /// not here yet: a put was interrupted, and its repeat finishes it.
    fn decode_social_message(&self, scalars: &Fields) -> StoreResult<Option<SocialMessage>> {
        let key = scalars.str(KEY)?;
        let Some(content) = self.find(CONTENT, KEY, &key)? else {
            return Ok(None);
        };
        let body: Body = content.json(RECORD)?;
        let malformed = || StoreError::corrupt(MESSAGE, scalars.id(), format!("key {key:?}"));
        let mut parts = key.splitn(3, SEP);
        let (Some(account), Some(place), Some(id)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(malformed());
        };
        let prefix = format!("{account}{SEP}{place}{SEP}");
        let parent = match scalars.opt_str("parent")? {
            Some(parent) => Some(
                parent
                    .strip_prefix(&prefix)
                    .ok_or_else(malformed)?
                    .to_owned(),
            ),
            None => None,
        };
        Ok(Some(SocialMessage {
            account: social_account_key(account),
            place: place.to_owned(),
            id: id.to_owned(),
            author: scalars.str("author")?,
            kind: scalars.str("kind")?,
            created: scalars.datetime("created")?,
            parent,
            deleted: scalars.bool("deleted")?,
            mentions_me: scalars.bool("mentions_me")?,
            unread: scalars.bool("unread")?,
            message: body.message,
            reactions: body.reactions,
        }))
    }
}
