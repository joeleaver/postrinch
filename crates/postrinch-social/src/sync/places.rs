//! The places a sync keeps: the channels of the person's groups, channels
//! followed without joining, and private conversations; and the groups.

use std::collections::HashSet;

use postrinch_ofscp::http::{Client, PageRequest};
use postrinch_ofscp::ofscp_core::wire::{Channel, DmConversation, Group, GroupPointer};
use postrinch_ofscp::ofscp_core::{Authority, DmId, LocalId};
use postrinch_store::{GroupFound, PlaceKind, SocialGroup, SocialPlace};
use serde_json::Value;

use super::{Reach, Run};
use crate::SocialResult;
use crate::uris::{channel_uri, group_uri, parse_channel, parse_group};

/// A place whose newest page a sync fetches.
pub(crate) struct Target {
    /// The place's id: a channel URI, or a dmId.
    pub place: String,
    /// The provider hosting it.
    pub host: Authority,
    pub source: Source,
    pub marker: Marker,
}

/// Where a place's messages are read from.
pub(crate) enum Source {
    /// A channel, from the provider hosting it.
    Channel {
        client: Client,
        group: LocalId,
        channel: LocalId,
    },
    /// A private conversation: from the home provider, and from the other
    /// person's when it is another, as each holds what was sent to it
    /// (§7.4, docs/ofscp.md "Private messages").
    Conversation { dm: DmId, other: Option<Client> },
}

/// What says which of a place's messages are read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Marker {
    /// The home provider's read marker: the newest message read, `None`
    /// when nothing is (§7.7).
    Provider(Option<String>),
    /// None from the provider (a channel elsewhere): the place keeps its
    /// own, and what is there when it is first seen counts as read.
    Local,
}

impl Run<'_> {
    /// Keep the groups and places, and give the places to fetch pages for.
    pub(super) async fn places(&mut self) -> SocialResult<Vec<Target>> {
        let home = self.home.clone();
        let follows = self.checked(&home, home.follows().await).await?;
        let home_authority = self.identity.actor().domain().clone();
        let mut followed: Vec<(String, crate::uris::ChannelAt)> = follows
            .follows
            .iter()
            .filter_map(|f| parse_channel(&f.channel, f.group_id.as_ref(), &home_authority))
            .map(|at| (channel_uri(&at.authority, &at.group, &at.channel), at))
            .collect();
        let followed_uris: HashSet<String> = followed.iter().map(|(uri, _)| uri.clone()).collect();

        let pointers = self.checked(&home, home.my_groups().await).await?;
        let mut targets = Vec::new();
        let mut groups = HashSet::new();
        for pointer in &pointers.groups {
            if let Some(uri) = self.group(pointer, &followed_uris, &mut targets).await? {
                groups.insert(uri);
            }
        }
        // Groups kept here that home leaves out: joined elsewhere, or asked.
        let outside = self.outside(&groups).await?;
        groups.extend(outside.spared);
        for pointer in &outside.pointers {
            if let Some(uri) = self.group(pointer, &followed_uris, &mut targets).await? {
                groups.insert(uri);
            }
        }
        self.forget_groups(&groups, &followed_uris).await?;
        // Channels followed in groups the person is not in, and their groups.
        followed.retain(|(uri, _)| !targets.iter().any(|t: &Target| &t.place == uri));
        let mut followed_groups = HashSet::new();
        for (uri, at) in followed {
            let client = self.client_for(&at.authority);
            let found = client.channel(&at.group, &at.channel).await;
            match self.reachable(&client, found).await? {
                Reach::Got(channel) => {
                    let group = group_uri(&at.authority, &at.group);
                    self.followed_group(&client, &at.group, &mut followed_groups)
                        .await?;
                    targets.push(self.channel(client, &group, &channel, true).await?);
                }
                Reach::Refused => {
                    // Kept as it is, and so is the group it is kept under.
                    self.spared.insert(uri);
                    self.spared.insert(group_uri(&at.authority, &at.group));
                }
                Reach::Gone => {}
            }
        }
        self.forget_followed_groups(&followed_groups).await?;
        let channels = targets.iter().map(|t| t.place.clone()).collect();
        self.forget_places(&channels).await?;
        let conversations = self
            .checked(&home, home.conversations(&PageRequest::newest()).await)
            .await?;
        for conversation in &conversations.items {
            targets.push(self.conversation(conversation).await?);
        }
        Ok(targets)
    }

    /// Keep a group the home provider lists the person in, with its
    /// channels; its URI, or `None` when it is gone or not a group. A
    /// group whose provider refused the signature is left as it is kept.
    pub(super) async fn group(
        &mut self,
        pointer: &GroupPointer,
        followed: &HashSet<String>,
        targets: &mut Vec<Target>,
    ) -> SocialResult<Option<String>> {
        let Some((authority, id)) = parse_group(&pointer.group) else {
            tracing::info!(group = %pointer.group, "not a group URI; left out");
            self.synced.skipped += 1;
            return Ok(None);
        };
        let uri = group_uri(&authority, &id);
        let client = self.client_for(&authority);
        let found = client.group(&id).await;
        let group = match self.reachable(&client, found).await? {
            Reach::Got(group) => group,
            Reach::Gone => {
                self.group_gone(pointer, &uri, followed).await?;
                return Ok(None);
            }
            Reach::Refused => {
                self.spared.insert(uri.clone());
                return Ok(Some(uri));
            }
        };
        self.keep_group(&uri, &authority, &group, pointer).await?;
        // Only the group's own answer says it is gone (docs/ofscp.md,
        // "Groups on other providers"): a channel list out of reach leaves
        // the group with no channel readable, and a refused one leaves its
        // channels as they are kept.
        let found = client.channels(&id).await;
        let channels = match self.reachable(&client, found).await? {
            Reach::Got(channels) => channels,
            Reach::Gone => Vec::new(),
            Reach::Refused => {
                self.spared.insert(uri.clone());
                Vec::new()
            }
        };
        for channel in &channels {
            let place = channel_uri(&authority, &channel.group_id, &channel.id);
            let is_followed = followed.contains(&place);
            targets.push(
                self.channel(client.clone(), &uri, channel, is_followed)
                    .await?,
            );
        }
        Ok(Some(uri))
    }

    async fn keep_group(
        &mut self,
        uri: &str,
        authority: &Authority,
        group: &Group,
        pointer: &GroupPointer,
    ) -> SocialResult<()> {
        let role = group
            .membership
            .as_ref()
            .map(|m| m.role.as_str().to_owned())
            .or_else(|| pointer.role.as_ref().map(|r| r.as_str().to_owned()));
        let (account, key) = (self.actor(), uri.to_owned());
        let kept = self
            .social
            .blocking(move |store| store.social_group(&account, &key))
            .await?;
        let record = SocialGroup {
            account: self.actor(),
            uri: uri.to_owned(),
            provider: authority.to_string(),
            group: to_json(group),
            role,
            found: GroupFound::Member,
            notices: kept.and_then(|g| g.notices),
        };
        self.social
            .blocking(move |store| store.put_social_group(&record))
            .await?;
        self.synced.groups += 1;
        Ok(())
    }

    /// Keep a channel as a place, and give it to fetch.
    async fn channel(
        &mut self,
        client: Client,
        group: &str,
        channel: &Channel,
        followed: bool,
    ) -> SocialResult<Target> {
        let host = client.provider().endpoint().authority().clone();
        let place = channel_uri(&host, &channel.group_id, &channel.id);
        let marker = if host == *self.identity.actor().domain() {
            self.marker(channel.id.as_str())
        } else {
            Marker::Local
        };
        let fresh = SocialPlace {
            account: self.actor(),
            id: place.clone(),
            kind: PlaceKind::Channel,
            group: Some(group.to_owned()),
            followed,
            last_activity: None,
            object: to_json(channel),
            cursors: Default::default(),
            read_marker: None,
            notices: None,
            muted: false,
        };
        self.keep_place(fresh).await?;
        Ok(Target {
            place,
            host,
            source: Source::Channel {
                client,
                group: channel.group_id.clone(),
                channel: channel.id.clone(),
            },
            marker,
        })
    }

    /// Keep a private conversation as a place, and give it to fetch.
    pub(crate) async fn conversation(
        &mut self,
        conversation: &DmConversation,
    ) -> SocialResult<Target> {
        let place = conversation.id.to_string();
        let fresh = SocialPlace {
            account: self.actor(),
            id: place.clone(),
            kind: PlaceKind::Conversation,
            group: None,
            followed: false,
            last_activity: Some(conversation.updated_at.as_millisecond()),
            object: to_json(conversation),
            cursors: Default::default(),
            read_marker: None,
            notices: None,
            muted: false,
        };
        self.keep_place(fresh).await?;
        let home = self.identity.actor().domain().clone();
        let other = conversation
            .participants
            .iter()
            .map(|p| p.domain())
            .find(|domain| **domain != home)
            .cloned()
            .map(|domain| self.client_for(&domain));
        Ok(Target {
            marker: self.marker(&place),
            place,
            host: home,
            source: Source::Conversation {
                dm: conversation.id.clone(),
                other,
            },
        })
    }

    /// The marker for a scope of the home provider's.
    fn marker(&self, scope: &str) -> Marker {
        match self.markers.get(scope) {
            Some(read) => Marker::Provider(read.clone()),
            None => Marker::Local,
        }
    }

    /// Keep `fresh`, keeping what a place already here holds of its own:
    /// its cursors, read marker, notices, muting and latest activity. The
    /// store merges under its lock, so a change the window makes meanwhile
    /// is kept.
    async fn keep_place(&mut self, fresh: SocialPlace) -> SocialResult<()> {
        self.social
            .blocking(move |store| {
                store.keep_social_place(fresh, |kept, fresh| SocialPlace {
                    last_activity: kept.last_activity.max(fresh.last_activity),
                    cursors: kept.cursors,
                    read_marker: kept.read_marker,
                    notices: kept.notices,
                    muted: kept.muted,
                    ..fresh
                })
            })
            .await?;
        self.synced.places += 1;
        Ok(())
    }
}

/// A wire object whole, as the store keeps it.
pub(super) fn to_json<T: serde::Serialize>(object: &T) -> Value {
    serde_json::to_value(object).expect("a wire object always serializes")
}
