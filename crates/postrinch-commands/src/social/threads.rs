//! A thread: a message and the messages that answer it, one level deep
//! (§7.2), as "N replies" opens it beside a stream.

use postrinch_store::{SocialPlace, social_account_key};
use serde::{Deserialize, Serialize};

use super::overlay::{PlaceOutbox, SocialItem, client_ids};
use super::places::SocialPlaceInfo;
use crate::{CommandResult, Commands};

/// A message that answers none, and its replies oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialThread {
    pub place: SocialPlaceInfo,
    pub root: SocialItem,
    pub replies: Vec<SocialItem>,
}

impl Commands {
    /// The thread `message` is in: itself and its replies, or the message
    /// it answers and that one's replies. A post still being sent is found
    /// by its client id. `None` for a message the place does not have.
    pub fn social_thread(
        &self,
        account: &str,
        place: &str,
        message: &str,
    ) -> CommandResult<Option<SocialThread>> {
        let place = self.check_place(account, place)?;
        let account = social_account_key(&place.account);
        let outbox = self.place_outbox(&account, &place.id)?;
        let Some(root) = self.thread_root(&place, &outbox, message)? else {
            return Ok(None);
        };
        let mut replies = Vec::new();
        if root.posted {
            let kept = self
                .store
                .social_replies(&account, &place.id, &root.message.id)?;
            let sent = client_ids(&kept);
            replies.extend(kept.into_iter().map(|m| outbox.lay_over(m)));
            replies.extend(outbox.queued(&account, &place.id, Some(&root.message.id), &sent));
        }
        replies.sort_by(|a, b| {
            (a.message.created, &a.message.id).cmp(&(b.message.created, &b.message.id))
        });
        let mut root = root;
        root.replies = replies.len();
        Ok(Some(SocialThread {
            place: self.place_info(place)?,
            root,
            replies,
        }))
    }

    /// The message that answers none of the thread `id` is in.
    fn thread_root(
        &self,
        place: &SocialPlace,
        outbox: &PlaceOutbox,
        id: &str,
    ) -> CommandResult<Option<SocialItem>> {
        let account = social_account_key(&place.account);
        let parent = match self.store.social_message(&account, &place.id, id)? {
            Some(message) => match message.parent.clone() {
                Some(parent) => parent,
                None => return Ok(Some(outbox.lay_over(message))),
            },
            None => match outbox.queued_post(id) {
                Some((_, post)) => match &post.reply_to {
                    Some(parent) => parent.clone(),
                    None => {
                        let kept = std::collections::HashSet::new();
                        return Ok(outbox
                            .queued(&account, &place.id, None, &kept)
                            .into_iter()
                            .find(|item| item.message.id == id));
                    }
                },
                None => return Ok(None),
            },
        };
        Ok(self
            .store
            .social_message(&account, &place.id, &parent)?
            .map(|m| outbox.lay_over(m)))
    }
}
