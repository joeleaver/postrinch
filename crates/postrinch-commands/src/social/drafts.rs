//! Drafts: posts, replies and messages being written, saved as the person
//! types and listed in Drafts (docs/ofscp.md "Writing"). Sending one is
//! [`super::posting`]'s.

use postrinch_store::{ContentKind, SocialDraft, SocialMessage, SocialPlace, social_account_key};

use super::{new_id, now};
use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// A new draft from `account`, for `place` when chosen, answering
    /// `reply_to` there when given. It is not kept until saved.
    pub fn start_social_draft(
        &self,
        account: &str,
        place: Option<&str>,
        reply_to: Option<&str>,
        kind: ContentKind,
    ) -> CommandResult<SocialDraft> {
        let account = self.check_social(account)?;
        let draft = SocialDraft {
            id: new_id(),
            account: social_account_key(&account.actor),
            place: place.map(str::to_owned),
            reply_to: reply_to.map(str::to_owned),
            updated: now(),
            kind,
            title: None,
            body: String::new(),
            tags: Vec::new(),
            cover: None,
            attachments: Vec::new(),
        };
        self.checked_draft(&draft)
    }

    /// Keep a draft as it is now, replacing the one with its id (a new id
    /// when it has none). Returns it as kept.
    pub fn save_social_draft(&self, draft: &SocialDraft) -> CommandResult<SocialDraft> {
        let mut draft = self.checked_draft(draft)?;
        if draft.id.is_empty() {
            draft.id = new_id();
        } else if let Some(kept) = self.store.social_draft(&draft.id)? {
            self.check_social(&kept.account)?;
        }
        draft.updated = now();
        self.store.put_social_draft(&draft)?;
        Ok(draft)
    }

    /// The drafts of every account this caller sees, the most recently
    /// changed first.
    pub fn social_drafts(&self) -> CommandResult<Vec<SocialDraft>> {
        let mut drafts = self.store.social_drafts()?;
        drafts.retain(|d| self.sees_social(&d.account));
        Ok(drafts)
    }

    /// The draft `id`; `None` when there is none this caller sees.
    pub fn social_draft(&self, id: &str) -> CommandResult<Option<SocialDraft>> {
        Ok(self
            .store
            .social_draft(id)?
            .filter(|d| self.sees_social(&d.account)))
    }

    /// Delete a draft. Returns whether there was one this caller sees.
    pub fn remove_social_draft(&self, id: &str) -> CommandResult<bool> {
        if self.social_draft(id)?.is_none() {
            return Ok(false);
        }
        Ok(self.store.remove_social_draft(id)?)
    }

    /// `draft` as it is kept: its account by key, its place one the account
    /// knows, and the message it answers one that answers none.
    pub(crate) fn checked_draft(&self, draft: &SocialDraft) -> CommandResult<SocialDraft> {
        let account = self.check_social(&draft.account)?;
        let mut draft = draft.clone();
        draft.account = social_account_key(&account.actor);
        match (&draft.place, &draft.reply_to) {
            (Some(place), reply_to) => {
                let place = self.check_place(&draft.account, place)?;
                if let Some(id) = reply_to {
                    draft.reply_to = Some(self.answered(&place, id)?.id);
                }
            }
            (None, Some(_)) => {
                return Err(CommandError::Invalid(
                    "A reply needs the place of the message it answers.".into(),
                ));
            }
            (None, None) => {}
        }
        Ok(draft)
    }

    /// The message a reply to `id` in `place` answers: `id` itself, or the
    /// message `id` answers, as replies go one level deep (§7.2).
    pub(crate) fn answered(&self, place: &SocialPlace, id: &str) -> CommandResult<SocialMessage> {
        let account = social_account_key(&place.account);
        let find = |id: &str| self.store.social_message(&account, &place.id, id);
        let Some(message) = find(id)? else {
            let outbox = self.place_outbox(&account, &place.id)?;
            return Err(CommandError::Invalid(if outbox.queued_post(id).is_some() {
                "That message is still being sent: answer it once it is.".into()
            } else {
                format!("There is no message {id} here to answer.")
            }));
        };
        match &message.parent {
            Some(parent) => find(parent)?.ok_or_else(|| {
                CommandError::Invalid(format!("The message {id} answers is not here."))
            }),
            None => Ok(message),
        }
    }
}
