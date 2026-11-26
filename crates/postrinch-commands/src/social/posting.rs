//! Posting, editing, deleting and reacting (docs/ofscp.md "Writing", §7.1).
//! Each is checked here against what the place permits, queued in the
//! outbox for the engine to send, and shown at once.
//!
//! These speak for the person to others: the window calls them on the
//! person's say. An agent proposes instead (docs/ofscp.md "Agents").

use postrinch_store::{
    ContentKind, SocialChange, SocialContent, SocialDraft, SocialMessage, SocialOutboxEntry,
    SocialPlace, SocialPost, social_account_key,
};

use super::overlay::SocialItem;
use super::{kind_of, now, same_actor};
use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// Send `draft` to its place: it shows there at once as being sent,
    /// and the draft kept with its id, if any, is gone.
    pub fn send_social(&self, draft: &SocialDraft) -> CommandResult<SocialItem> {
        if let Some(kept) = self.store.social_draft(&draft.id)? {
            self.check_social(&kept.account)?;
        }
        let draft = self.checked_draft(draft)?;
        let Some(place) = &draft.place else {
            return Err(CommandError::Invalid("Choose where it goes first.".into()));
        };
        let place = self.check_place(&draft.account, place)?;
        let parent = match &draft.reply_to {
            Some(id) => Some(self.answered(&place, id)?),
            None => None,
        };
        self.standing(&place)?
            .check_post(draft.kind, parent.as_ref())?;
        let content = SocialContent {
            body: draft.body.clone(),
            title: draft.title.clone(),
            tags: draft.tags.clone(),
            cover: draft.cover.clone(),
            attachments: draft.attachments.clone(),
        };
        let post = SocialPost {
            kind: draft.kind,
            reply_to: parent.map(|p| p.id),
            content: self.checked_content(&draft.account, draft.kind, content)?,
        };
        let entry =
            self.queue_change(&draft.account, Some(&place.id), &SocialChange::Post(post))?;
        self.store.remove_social_draft(&draft.id)?;
        self.message_item(&place, &entry.client_id)
    }

    /// Give a message of the account's own new content (§7.1 *Editing*:
    /// until its `editUntil`, or always when it has none).
    pub fn edit_social_message(
        &self,
        account: &str,
        place: &str,
        message: &str,
        content: &SocialContent,
    ) -> CommandResult<SocialItem> {
        let place = self.check_place(account, place)?;
        let kept = self.posted(&place, message)?;
        if !same_actor(&kept.author, &place.account) {
            return Err(CommandError::Invalid(
                "You can edit only what you wrote.".into(),
            ));
        }
        if let Some(until) = kept
            .message
            .pointer("/permissions/editUntil")
            .and_then(serde_json::Value::as_str)
            && until
                .parse::<jiff::Timestamp>()
                .is_ok_and(|until| until.as_millisecond() <= now())
        {
            return Err(CommandError::Invalid(
                "It can't be edited any more: its time for edits is over.".into(),
            ));
        }
        let Some(kind) = kind_of(&kept.kind) else {
            return Err(CommandError::Invalid(format!(
                "Postrinch can't edit a {:?} message.",
                kept.kind
            )));
        };
        let content = self.checked_content(&place.account, kind, content.clone())?;
        let change = SocialChange::Edit {
            message: kept.id.clone(),
            content,
        };
        self.queue_change(&place.account, Some(&place.id), &change)?;
        self.message_item(&place, &kept.id)
    }

    /// Delete a message, the account's own or (with `moderate`) another's,
    /// leaving its tombstone. A post not yet tried is taken back instead,
    /// and `None` returned.
    pub fn delete_social_message(
        &self,
        account: &str,
        place: &str,
        message: &str,
    ) -> CommandResult<Option<SocialItem>> {
        let place = self.check_place(account, place)?;
        let account = social_account_key(&place.account);
        if let Some((entry, _)) = self.place_outbox(&account, &place.id)?.queued_post(message) {
            if entry.attempts > 0 {
                return Err(CommandError::Invalid(
                    "It may have reached its provider already: delete it once it shows as sent, or give it up.".into(),
                ));
            }
            self.store
                .remove_social_change(&account, &entry.client_id)?;
            return Ok(None);
        }
        let kept = self.posted(&place, message)?;
        if !same_actor(&kept.author, &account) && !self.standing(&place)?.may_moderate() {
            return Err(CommandError::Invalid(
                "You can delete only what you wrote here.".into(),
            ));
        }
        let change = SocialChange::Delete {
            message: kept.id.clone(),
        };
        self.queue_change(&account, Some(&place.id), &change)?;
        self.message_item(&place, &kept.id).map(Some)
    }

    /// Add the reaction `key` (an emoji, or a custom reaction's name) to a
    /// message, or take it back.
    pub fn react_social(
        &self,
        account: &str,
        place: &str,
        message: &str,
        key: &str,
        on: bool,
    ) -> CommandResult<SocialItem> {
        let place = self.check_place(account, place)?;
        let key = key.trim();
        if key.is_empty() {
            return Err(CommandError::Invalid("A reaction needs an emoji.".into()));
        }
        let kept = self.posted(&place, message)?;
        if !self.standing(&place)?.may_react() {
            return Err(CommandError::Invalid("You can't react here.".into()));
        }
        let shown = self.message_item(&place, &kept.id)?;
        let mine = shown
            .message
            .reactions
            .iter()
            .any(|r| r.key == key && r.mine);
        if mine == on {
            return Ok(shown);
        }
        let change = SocialChange::React {
            message: kept.id.clone(),
            key: key.to_owned(),
            on,
        };
        self.queue_change(&place.account, Some(&place.id), &change)?;
        self.message_item(&place, &kept.id)
    }

    /// The account's changes still on their way, in the order made, each
    /// with how often sending it failed and why.
    pub fn social_outbox(&self, account: &str) -> CommandResult<Vec<SocialOutboxEntry>> {
        let account = self.check_social(account)?;
        Ok(self.store.social_outbox(&account.actor)?)
    }

    /// Give up a change still on its way, as the person may once sending
    /// it fails. Returns whether it was queued.
    pub fn give_up_social_change(&self, account: &str, client_id: &str) -> CommandResult<bool> {
        let account = self.check_social(account)?;
        Ok(self.store.remove_social_change(&account.actor, client_id)?)
    }

    /// The message `id` of a place as shown, or the post being sent with
    /// that client id.
    pub(crate) fn message_item(&self, place: &SocialPlace, id: &str) -> CommandResult<SocialItem> {
        let account = social_account_key(&place.account);
        let outbox = self.place_outbox(&account, &place.id)?;
        let mut item = match self.store.social_message(&account, &place.id, id)? {
            Some(message) => outbox.lay_over(message, &account),
            None => {
                let parent = outbox
                    .queued_post(id)
                    .and_then(|(_, post)| post.reply_to.clone());
                let kept = std::collections::HashSet::new();
                outbox
                    .queued(&account, &place.id, parent.as_deref(), &kept)
                    .into_iter()
                    .find(|item| item.message.id == id)
                    .ok_or_else(|| {
                        CommandError::Invalid(format!("There is no message {id} here."))
                    })?
            }
        };
        self.count_replies(&outbox, std::slice::from_mut(&mut item))?;
        Ok(item)
    }

    /// The message `id` its provider has, not deleted.
    fn posted(&self, place: &SocialPlace, id: &str) -> CommandResult<SocialMessage> {
        let item = self.message_item(place, id)?;
        if !item.posted {
            return Err(CommandError::Invalid(
                "That message is still being sent: wait until it is.".into(),
            ));
        }
        if item.message.deleted {
            return Err(CommandError::Invalid("That message is deleted.".into()));
        }
        Ok(item.message)
    }

    /// `content` as it is sent for a `kind`: trimmed, its tags once each,
    /// something to say, the article fields on articles alone (§5.3
    /// *Article fields*), and its media ready to upload.
    fn checked_content(
        &self,
        actor: &str,
        kind: ContentKind,
        mut content: SocialContent,
    ) -> CommandResult<SocialContent> {
        content.body = content.body.trim_end().to_owned();
        content.title = content
            .title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        let mut tags: Vec<String> = Vec::new();
        for tag in &content.tags {
            let tag = tag.trim();
            if !tag.is_empty() && !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                tags.push(tag.to_owned());
            }
        }
        content.tags = tags;
        if kind != ContentKind::Article && (content.title.is_some() || content.cover.is_some()) {
            return Err(CommandError::Invalid(
                "Only an article has a title and a cover.".into(),
            ));
        }
        if content.body.trim().is_empty() && content.attachments.is_empty() {
            return Err(CommandError::Invalid("There is nothing to send.".into()));
        }
        if let Some(cover) = &content.cover
            && !super::media::is_picture(cover)
        {
            return Err(CommandError::Invalid("A cover must be a picture.".into()));
        }
        for media in content.media() {
            self.check_upload(actor, media)?;
        }
        Ok(content)
    }
}
