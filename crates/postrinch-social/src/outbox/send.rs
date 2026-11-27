//! Sending a queued message change: a post or reply, an edit, a delete, a
//! reaction (§7.1, §7.4), and keeping what its provider answers.
//!
//! Every request is signed afresh, so a retry is a new request the
//! provider checks again (§4.5); what makes a retry safe is the change
//! itself. A post carries its client id as `clientMessageId`, and a
//! provider answers a repeat with the message the first made (`200`); an
//! edit sets the same content again; deleting a tombstone and taking back
//! a reaction that is not there both succeed; adding a reaction already
//! there answers it unchanged.

use std::sync::Arc;

use postrinch_ofscp::Identity;
use postrinch_ofscp::http::{Client, PageRequest};
use postrinch_ofscp::ofscp_core::wire::{
    ConversationPointer, Message, MessageReference, Reaction, ReactionPut, ReferenceKind,
};
use postrinch_ofscp::ofscp_core::{Actor, Authority, DmId};
use postrinch_store::{SocialContent, SocialPlace, SocialPost};

use super::content::{self, NotReady};
use super::outcome::{Outcome, is_not_found, outcome_of};
use super::target::{MessageAt, Target};
use crate::live::apply::{self, Arrival, Keeper, collect};
use crate::{Social, SocialError, SocialResult};

/// An account sending its changes.
pub(crate) struct Sender<'a> {
    pub social: &'a Social,
    pub identity: &'a Arc<Identity>,
}

/// A queued change about to go: the place it goes to, and where that is.
pub(crate) struct Going<'a> {
    pub client_id: &'a str,
    pub place: &'a SocialPlace,
    pub target: &'a Target,
}

impl From<NotReady> for Outcome {
    fn from(not_ready: NotReady) -> Self {
        match not_ready {
            NotReady::Waiting => Outcome::Waiting,
            NotReady::Damaged(error) => Outcome::Refused(error),
        }
    }
}

/// What a change that names a message it cannot place is.
fn no_such_message(kept: &str) -> Outcome {
    Outcome::Refused(format!("{kept:?} names no message this place can reach"))
}

/// The outcome of a change's requests: what they did, or what their
/// failure means.
pub(crate) fn settled(result: SocialResult<Outcome>) -> SocialResult<Outcome> {
    result.or_else(outcome_of)
}

impl Sender<'_> {
    pub(crate) fn actor(&self) -> &Actor {
        self.identity.actor()
    }

    pub(crate) fn home(&self) -> &Authority {
        self.actor().domain()
    }

    pub(crate) fn client(&self, authority: &Authority) -> Client {
        self.social.client(self.identity, authority)
    }

    /// A request's answer: a signature home refuses marks the account.
    pub(crate) async fn checked<T>(
        &self,
        client: &Client,
        result: postrinch_ofscp::Result<T>,
    ) -> SocialResult<T> {
        self.social.checked(self.identity, client, result).await
    }

    /// Who keeps the answers for a place hosted at `host`.
    pub(crate) fn keeper<'k>(&'k self, host: &'k Authority) -> Keeper<'k> {
        Keeper {
            social: self.social,
            actor: self.actor(),
            host,
        }
    }

    /// Post `post` (§7.1 *Sending messages*, §7.4).
    pub(crate) async fn post(&self, going: &Going<'_>, post: &SocialPost) -> SocialResult<Outcome> {
        let parent = match &post.reply_to {
            Some(kept) => match going.target.message(kept, self.home()) {
                Some(parent) => Some(parent),
                None => return Ok(no_such_message(kept)),
            },
            None => None,
        };
        match going.target {
            Target::Channel(at) => {
                // A reply in a channel answers a message the channel holds:
                // its provider is left out (§5.3).
                let reference = parent.map(|p| reply_to(p, None));
                let create = match content::channel_post(going.client_id, post, reference) {
                    Ok(create) => create,
                    Err(not_ready) => return Ok(not_ready.into()),
                };
                let client = self.client(&at.authority);
                let posted = client.post_message(&at.group, &at.channel, &create).await;
                let posted = self.checked(&client, posted).await?;
                self.keep(going, &at.authority, posted.into_message())
                    .await?;
            }
            Target::Conversation { dm, other } => {
                // Sent to the recipient's home, which holds it (§7.4). A
                // reply whose parent the other side holds names that
                // provider, as parents are keyed by provider and id.
                let to = other.domain();
                let reference = parent.map(|p| {
                    let provider = (&p.holder != to).then(|| p.holder.clone());
                    reply_to(p, provider)
                });
                let create = match content::dm_post(going.client_id, other, post, reference) {
                    Ok(create) => create,
                    Err(not_ready) => return Ok(not_ready.into()),
                };
                let client = self.client(to);
                let posted = client.send_dm(dm, &create).await;
                let message = self.checked(&client, posted).await?.into_message();
                self.keep(going, &self.home().clone(), message.clone())
                    .await?;
                if to != self.home() {
                    self.point_home(dm, other, message).await;
                }
            }
        }
        Ok(Outcome::Done)
    }

    /// Keep a message its provider answered with, in the place hosted at
    /// `host`. A post's copy settles it in the store by its client id.
    async fn keep(
        &self,
        going: &Going<'_>,
        host: &Authority,
        message: Message,
    ) -> SocialResult<()> {
        let place = going.place.id.clone();
        apply::message(&self.keeper(host), place, message, None, Arrival::Any).await
    }

    /// Put the conversation's pointer at home after a message went to the
    /// other person's provider (§7.4 *Conversation pointers*), so the
    /// account's other devices find it. Best effort: home makes one from
    /// the next message it holds all the same.
    async fn point_home(&self, dm: &DmId, other: &Actor, message: Message) {
        let home = self.client(self.home());
        let pointer = ConversationPointer {
            with: other.clone(),
            last_message: Some(message),
        };
        let put = home.put_conversation_pointer(dm, &pointer).await;
        if let Err(error) = self.checked(&home, put).await {
            tracing::warn!(actor = %self.actor(), %dm, %error, "no conversation pointer put at home");
        }
    }

    /// Give a message of the account's new content (§7.1 *Editing*).
    pub(crate) async fn edit(
        &self,
        going: &Going<'_>,
        kept: &str,
        content: &SocialContent,
    ) -> SocialResult<Outcome> {
        let Some(at) = going.target.message(kept, self.home()) else {
            return Ok(no_such_message(kept));
        };
        let article = self.is_article(going.place, kept).await?;
        let in_channel = matches!(going.target, Target::Channel(_));
        let update = match content::edit(content, article, in_channel) {
            Ok(update) => update,
            Err(not_ready) => return Ok(not_ready.into()),
        };
        let (host, edited) = match going.target {
            Target::Channel(channel) => {
                let client = self.client(&channel.authority);
                let edited = client
                    .edit_message(&channel.group, &channel.channel, &at.id, &update)
                    .await;
                (
                    channel.authority.clone(),
                    self.checked(&client, edited).await?,
                )
            }
            // Edited at the provider holding it, by federation (§7.4).
            Target::Conversation { dm, .. } => {
                let client = self.client(&at.holder);
                let edited = client.edit_dm(dm, &at.id, &update).await;
                (self.home().clone(), self.checked(&client, edited).await?)
            }
        };
        self.keep(going, &host, edited).await?;
        Ok(Outcome::Done)
    }

    /// Whether the kept message `kept` is an article: its title and cover
    /// are edited with it.
    pub(crate) async fn is_article(&self, place: &SocialPlace, kept: &str) -> SocialResult<bool> {
        let (account, place, id) = (self.actor().to_string(), place.id.clone(), kept.to_owned());
        let message = self
            .social
            .blocking(move |store| store.social_message(&account, &place, &id))
            .await?;
        Ok(message.is_some_and(|m| m.kind == "article"))
    }

    /// Delete a message, leaving its tombstone (§7.1 *Deleting*). One
    /// already gone is deleted.
    pub(crate) async fn delete(&self, going: &Going<'_>, kept: &str) -> SocialResult<Outcome> {
        let Some(at) = going.target.message(kept, self.home()) else {
            return Ok(no_such_message(kept));
        };
        let (client, deleted) = match going.target {
            Target::Channel(channel) => {
                let client = self.client(&channel.authority);
                let deleted = client
                    .delete_message(&channel.group, &channel.channel, &at.id)
                    .await;
                (client, deleted)
            }
            Target::Conversation { dm, .. } => {
                let client = self.client(&at.holder);
                let deleted = client.delete_dm(dm, &at.id).await;
                (client, deleted)
            }
        };
        match self.checked(&client, deleted).await {
            Ok(()) => {}
            Err(error) if is_not_found(&error) => {}
            Err(error) => return Err(error),
        }
        let host = going.target.host(self.home());
        let now = client.provider().clock().now();
        let place = going.place.id.clone();
        apply::deleted(&self.keeper(&host), place, kept.to_owned(), now, None).await?;
        Ok(Outcome::Done)
    }

    /// Add or take back the account's reaction `key` (§7.1 *Reactions*),
    /// then read the message's reactions again, all of them. A reaction is
    /// held with its message, at the provider holding that (§7.4).
    ///
    /// Once the provider has the change it is done, whatever reading the
    /// reactions back then meets: sending it again would change nothing,
    /// and the provider's `message.reaction` event, or the next sync,
    /// reads them again.
    pub(crate) async fn react(
        &self,
        going: &Going<'_>,
        kept: &str,
        key: &str,
        on: bool,
    ) -> SocialResult<Outcome> {
        let Some(at) = going.target.message(kept, self.home()) else {
            return Ok(no_such_message(kept));
        };
        let client = self.client(&at.holder);
        let looks = ReactionPut::default();
        let changed = match (going.target, on) {
            (Target::Channel(c), true) => client
                .put_reaction(&c.group, &c.channel, &at.id, key, &looks)
                .await
                .map(drop),
            (Target::Channel(c), false) => {
                client
                    .remove_reaction(&c.group, &c.channel, &at.id, key)
                    .await
            }
            (Target::Conversation { dm, .. }, true) => client
                .put_dm_reaction(dm, &at.id, key, &looks)
                .await
                .map(drop),
            (Target::Conversation { dm, .. }, false) => {
                client.remove_dm_reaction(dm, &at.id, key).await
            }
        };
        match self.checked(&client, changed).await {
            Ok(()) => {}
            // Taking back a reaction on a message that is gone: there is
            // nothing left to take it from.
            Err(error) if !on && is_not_found(&error) => return Ok(Outcome::Done),
            Err(error) => return Err(error),
        }
        let all = match self.reactions(going.target, &client, &at).await {
            Ok(all) => all,
            Err(error @ (SocialError::Transport(_) | SocialError::Socket(_))) => {
                tracing::info!(client_id = going.client_id, %error, "a reaction was sent; its message's reactions could not be read back");
                return Ok(Outcome::Done);
            }
            Err(error) => return Err(error),
        };
        let host = going.target.host(self.home());
        let place = going.place.id.clone();
        apply::reactions(&self.keeper(&host), place, kept.to_owned(), all).await?;
        Ok(Outcome::Done)
    }

    /// All of a message's reactions, from `client`, the provider holding
    /// it.
    async fn reactions(
        &self,
        target: &Target,
        client: &Client,
        at: &MessageAt,
    ) -> SocialResult<Vec<Reaction>> {
        let mut all = Vec::new();
        let mut request = PageRequest::newest();
        loop {
            let found = match target {
                Target::Channel(c) => {
                    client
                        .reactions(&c.group, &c.channel, &at.id, &request)
                        .await
                }
                Target::Conversation { dm, .. } => client.dm_reactions(dm, &at.id, &request).await,
            };
            let page = self.checked(client, found).await?;
            match collect(&mut all, page) {
                Some(next) => request = next,
                None => return Ok(all),
            }
        }
    }
}

/// A reply's reference to `parent`, naming its provider when given.
fn reply_to(parent: MessageAt, provider: Option<Authority>) -> MessageReference {
    MessageReference {
        kind: ReferenceKind::Reply,
        id: parent.id,
        provider,
    }
}
