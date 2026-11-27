//! Sending the queued changes the home provider keeps for the account: a
//! read marker (§7.7) and a follow (§7.6).

use postrinch_ofscp::ofscp_core::wire::{
    FollowCreateRequest, ReadMarkerUpdate, ReadMarkersUpdateRequest,
};
use postrinch_ofscp::ofscp_core::{LocalId, ObjectRef, ObjectUri};

use super::outcome::{Outcome, is_not_found};
use super::send::{Going, Sender};
use super::target::Target;
use crate::SocialResult;
use crate::live::{Control, LiveEvent};
use crate::uris::{channel_uri, group_uri, parse_channel, parse_dm_message_url};

impl Sender<'_> {
    /// Move the place's read marker at home to `kept` (§7.7). Home keeps
    /// markers for the places it holds: its own channels, by their local
    /// id, and conversations, by their dmId, up to a message it holds.
    /// Elsewhere read state is kept here alone, and nothing is sent.
    pub(crate) async fn read(&self, going: &Going<'_>, kept: &str) -> SocialResult<Outcome> {
        let scope = match going.target {
            Target::Channel(at) if &at.authority == self.home() => at.channel.clone(),
            Target::Conversation { dm, .. } if parse_dm_message_url(kept).is_none() => {
                match LocalId::new(dm.as_str()) {
                    Ok(scope) => scope,
                    Err(_) => return Ok(Outcome::Done),
                }
            }
            _ => return Ok(Outcome::Done),
        };
        let Ok(message) = LocalId::new(kept) else {
            return Ok(Outcome::Refused(format!("{kept:?} is not a message id")));
        };
        let request = ReadMarkersUpdateRequest {
            markers: vec![ReadMarkerUpdate {
                scope_id: scope,
                last_read_message_id: message,
            }],
        };
        let home = self.client(self.home());
        let moved = home.update_read_markers(&request).await;
        self.checked(&home, moved).await?;
        Ok(Outcome::Done)
    }

    /// Follow the channel, or stop following it (§7.6), at home; then keep
    /// that, and have the live account start or stop the session its
    /// provider needs.
    pub(crate) async fn follow(&self, going: &Going<'_>, on: bool) -> SocialResult<Outcome> {
        let Target::Channel(at) = going.target else {
            return Ok(Outcome::Refused("only a channel is followed".into()));
        };
        let home = self.client(self.home());
        if on {
            // A channel of home is named by its local id (§2.2), any
            // other by its URI, each with its group.
            let request = if &at.authority == self.home() {
                FollowCreateRequest {
                    channel: ObjectRef::Local(at.channel.clone()),
                    group_id: Some(ObjectRef::Local(at.group.clone())),
                }
            } else {
                let channel = channel_uri(&at.authority, &at.group, &at.channel);
                let group = group_uri(&at.authority, &at.group);
                match (ObjectUri::parse(&channel), ObjectUri::parse(&group)) {
                    (Ok(channel), Ok(group)) => FollowCreateRequest {
                        channel: ObjectRef::Uri(channel),
                        group_id: Some(ObjectRef::Uri(group)),
                    },
                    _ => {
                        return Ok(Outcome::Refused(format!(
                            "{channel} is not a channel's URI"
                        )));
                    }
                }
            };
            let followed = home.follow(&request).await;
            self.checked(&home, followed).await?;
        } else {
            // A follow is taken back by the ref it was made with (§7.6),
            // which home lists.
            let follows = home.follows().await;
            let follows = self.checked(&home, follows).await?;
            for follow in follows.follows {
                let names = parse_channel(&follow.channel, follow.group_id.as_ref(), self.home());
                if names.as_ref() != Some(at) {
                    continue;
                }
                let unfollowed = home.unfollow(&follow.channel).await;
                match self.checked(&home, unfollowed).await {
                    Ok(()) => {}
                    Err(error) if is_not_found(&error) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        self.kept_followed(going, on).await?;
        Ok(Outcome::Done)
    }

    /// Keep that the place is followed, or not, and say so.
    async fn kept_followed(&self, going: &Going<'_>, on: bool) -> SocialResult<()> {
        let (account, place) = (self.actor().to_string(), going.place.id.clone());
        let changed = self
            .social
            .blocking(move |store| {
                let mut changed = false;
                store.change_social_place(&account, &place, |p| {
                    changed = p.followed != on;
                    p.followed = on;
                })?;
                Ok(changed)
            })
            .await?;
        if changed {
            let account = self.actor().to_string();
            self.social.live.tell(LiveEvent::Place {
                account: account.clone(),
                place: going.place.id.clone(),
            });
            let key = postrinch_store::social_account_key(&account);
            self.social.live.control(&key, Control::Refreshed);
        }
        Ok(())
    }
}
