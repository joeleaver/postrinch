//! The outbox (docs/ofscp.md, "Outbox"; the plan's PR 10): the changes
//! the commands queue in the store ([`postrinch_store::SocialChange`]),
//! sent to their providers.
//!
//! A pass ([`Social::send_outbox`]) goes through an account's queue in
//! the order the changes were made:
//!
//! - **In order, per place.** A change that fails for now (the provider
//!   cannot be reached, or fails) is tried again later, and the changes
//!   after it in its place wait for it; other places go on. A change
//!   carrying a file not yet uploaded waits too, untried.
//! - **Refused for good** (a `403`, a `400`, a `404` for an edit of what
//!   is gone): the change is kept, with why, for the person to give up,
//!   and holds back nothing; it is not tried again.
//! - **Retries are safe.** The queue is in the store, so it survives a
//!   dropped connection and a restart. A change is marked tried before
//!   its request goes, and each request is signed afresh. A post carries
//!   its client id as `clientMessageId`; whether its first send reached
//!   the provider or not, its provider's copy is matched to it by that id
//!   (a repeat is answered `200` with the first copy), and keeping the
//!   copy settles the queued post ([`send`]).
//! - **What each provider answers is kept** as the live engine keeps
//!   events ([`crate::live::apply`]): a sent message, an edit, a
//!   tombstone, a message's reactions read again.
//!
//! A live account runs passes by itself ([`run`]): when it goes live, when
//! told there is more ([`Social::outbox_changed`]), when a session catches
//! up after connecting, and again with backoff while a change fails.
//!
//! Notices settings and the profile are queued by the commands but sent
//! by the engines of those features (the plan's rows 14 and 22): a pass
//! leaves them queued and untried.

mod content;
mod marks;
mod outcome;
pub(crate) mod run;
mod send;
mod target;

use std::collections::HashSet;
use std::time::Duration;

use postrinch_ofscp::ofscp_core::Actor;
use postrinch_store::{SocialChange, SocialContent, SocialOutboxEntry, social_account_key};

use crate::live::{Control, LiveEvent};
use crate::{Social, SocialResult, parse_actor};
use outcome::Outcome;
use send::{Going, Sender, settled};
use target::Target;

/// What a pass over an account's outbox did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sent {
    /// Changes their providers took: gone from the outbox.
    pub sent: usize,
    /// Changes that failed for now, to be tried again.
    pub failed: usize,
    /// Changes their providers refused for good, kept for the person.
    pub refused: usize,
    /// Changes waiting for their files to be uploaded.
    pub waiting: usize,
    /// The longest wait a failing provider asked for (`Retry-After`).
    pub retry_after: Option<Duration>,
}

impl Social {
    /// Send `actor`'s queued changes, as far as they go now (see the
    /// module's docs). Passes over one account run one at a time.
    pub async fn send_outbox(&self, actor: &str) -> SocialResult<Sent> {
        self.send_outbox_of(&parse_actor(actor)?).await
    }

    /// Say that `actor`'s outbox has new changes: a live account sends
    /// them now. Nothing happens for an account that is not live; its
    /// changes go when it is, or by [`Social::send_outbox`].
    pub fn outbox_changed(&self, actor: &str) {
        if let Ok(actor) = parse_actor(actor) {
            let key = social_account_key(&actor.to_string());
            self.live.control(&key, Control::Outbox);
        }
    }

    pub(crate) async fn send_outbox_of(&self, actor: &Actor) -> SocialResult<Sent> {
        let _pass = self
            .passes
            .hold(social_account_key(&actor.to_string()))
            .await;
        let account = self.account(actor).await?;
        let identity = self.identity(&account).await?;
        let name = account.actor.clone();
        let entries = self
            .blocking(move |store| store.social_outbox(&name))
            .await?;
        let sender = Sender {
            social: self,
            identity: &identity,
        };
        let mut sent = Sent::default();
        let mut held_back: HashSet<String> = HashSet::new();
        for entry in entries {
            if entry.refused || entry.place.as_ref().is_some_and(|p| held_back.contains(p)) {
                continue;
            }
            let Some(outcome) = self.send_entry(&sender, &entry).await? else {
                continue;
            };
            if outcome.holds_back()
                && let Some(place) = &entry.place
            {
                held_back.insert(place.clone());
            }
            self.record(&account.actor, &entry, outcome, &mut sent)
                .await?;
        }
        Ok(sent)
    }

    /// Send one queued change; `None` when it was taken back meanwhile.
    async fn send_entry(
        &self,
        sender: &Sender<'_>,
        entry: &SocialOutboxEntry,
    ) -> SocialResult<Option<Outcome>> {
        if let Some(outcome) = unsendable(&entry.change) {
            return Ok(Some(outcome));
        }
        let Some(place_id) = entry.place.clone() else {
            return Ok(Some(Outcome::Refused("the change names no place".into())));
        };
        // Held for each change, so signing out or in again waits for the
        // one being sent, but never for the whole pass.
        let _account = self.hold(sender.actor()).await;
        let account = entry.account.clone();
        let place = self
            .blocking(move |store| store.social_place(&account, &place_id))
            .await?;
        let Some(place) = place else {
            return Ok(Some(Outcome::Refused("its place is no longer kept".into())));
        };
        let Some(target) = Target::of(&place, sender.actor()) else {
            let why = format!("{} cannot be sent to", place.id);
            return Ok(Some(Outcome::Refused(why)));
        };
        let (account, client_id) = (entry.account.clone(), entry.client_id.clone());
        let tried = self
            .blocking(move |store| store.social_change_tried(&account, &client_id))
            .await?;
        if tried.is_none() {
            return Ok(None);
        }
        let going = Going {
            client_id: &entry.client_id,
            place: &place,
            target: &target,
        };
        let sent = match &entry.change {
            SocialChange::Post(post) => sender.post(&going, post).await,
            SocialChange::Edit { message, content } => sender.edit(&going, message, content).await,
            SocialChange::Delete { message } => sender.delete(&going, message).await,
            SocialChange::React { message, key, on } => {
                sender.react(&going, message, key, *on).await
            }
            SocialChange::Read { message } => sender.read(&going, message).await,
            SocialChange::Follow { on, .. } => sender.follow(&going, *on).await,
            SocialChange::ChannelNotices { .. }
            | SocialChange::GroupNotices { .. }
            | SocialChange::Profile(_) => Ok(Outcome::NotSentHere),
        };
        settled(sent).map(Some)
    }

    /// Keep what came of `entry`, count it, and say so.
    ///
    /// A post leaves the outbox as its copy is kept, which tells it sent
    /// ([`crate::live::apply::message`]); it may arrive on a socket while
    /// its own answer is lost, so a failure is not kept for a post no
    /// longer queued: it is sent. Any other change no longer queued was
    /// given up.
    async fn record(
        &self,
        account: &str,
        entry: &SocialOutboxEntry,
        outcome: Outcome,
        sent: &mut Sent,
    ) -> SocialResult<()> {
        let (name, client_id) = (account.to_owned(), entry.client_id.clone());
        let gone = if matches!(entry.change, SocialChange::Post(_)) {
            Recorded::SentAlready
        } else {
            Recorded::Nothing
        };
        let recorded = match outcome {
            Outcome::Done => {
                let removed = self
                    .blocking(move |store| store.remove_social_change(&name, &client_id))
                    .await?;
                if removed { Recorded::Sent } else { gone }
            }
            Outcome::Again { error, retry_after } => {
                let why = error.clone();
                let kept = self
                    .blocking(move |store| store.social_change_failed(&name, &client_id, &why))
                    .await?;
                if kept.is_none() {
                    gone
                } else {
                    sent.retry_after = sent.retry_after.max(retry_after);
                    tracing::info!(%account, client_id = %entry.client_id, %error, "a change failed; it is tried again");
                    Recorded::NotSent {
                        error,
                        refused: false,
                    }
                }
            }
            Outcome::Refused(error) => {
                let why = error.clone();
                let kept = self
                    .blocking(move |store| store.social_change_refused(&name, &client_id, &why))
                    .await?;
                if kept.is_none() {
                    gone
                } else {
                    tracing::warn!(%account, client_id = %entry.client_id, %error, "a change was refused");
                    Recorded::NotSent {
                        error,
                        refused: true,
                    }
                }
            }
            Outcome::Waiting => {
                sent.waiting += 1;
                Recorded::Nothing
            }
            Outcome::NotSentHere => Recorded::Nothing,
        };
        let (account, client_id, place) = (
            account.to_owned(),
            entry.client_id.clone(),
            entry.place.clone(),
        );
        match recorded {
            Recorded::Sent => {
                sent.sent += 1;
                self.live.tell(LiveEvent::Sent {
                    account,
                    client_id,
                    place,
                });
            }
            Recorded::SentAlready => sent.sent += 1,
            Recorded::NotSent { error, refused } => {
                if refused {
                    sent.refused += 1;
                } else {
                    sent.failed += 1;
                }
                self.live.tell(LiveEvent::NotSent {
                    account,
                    client_id,
                    place,
                    error,
                    refused,
                });
            }
            Recorded::Nothing => {}
        }
        Ok(())
    }
}

/// What a pass says of a change once its outcome is kept.
enum Recorded {
    /// It reached its provider.
    Sent,
    /// It reached its provider, and keeping its copy said so.
    SentAlready,
    /// It did not, and why; `refused` when it never will.
    NotSent { error: String, refused: bool },
    /// Nothing to say: it waits, is another engine's, or was given up.
    Nothing,
}

/// What a change that cannot go as it is comes to, before anything is
/// tried: one carrying a file not yet uploaded waits, and one of the
/// features another engine sends is left for it.
fn unsendable(change: &SocialChange) -> Option<Outcome> {
    let content_of = |content: &SocialContent| content::ready(content).err().map(Outcome::from);
    match change {
        SocialChange::Post(post) => content_of(&post.content),
        SocialChange::Edit { content, .. } => content_of(content),
        SocialChange::ChannelNotices { .. }
        | SocialChange::GroupNotices { .. }
        | SocialChange::Profile(_) => Some(Outcome::NotSentHere),
        SocialChange::Delete { .. }
        | SocialChange::React { .. }
        | SocialChange::Read { .. }
        | SocialChange::Follow { .. } => None,
    }
}
