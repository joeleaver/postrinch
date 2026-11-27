//! Everything a fake provider holds, in memory. One lock guards it all;
//! it is never held across an `.await`.

pub(crate) mod groups;
pub(crate) mod timeline;

use std::collections::{BTreeMap, HashMap};

use bytes::Bytes;
use jiff::Timestamp;
use ofscp_core::signing::VerifyingKey;
use ofscp_core::wire::{Attachment, Availability, Follow, GroupPointer, Message, Notification};
use ofscp_core::{Actor, Authority, DmId, LocalId};

use crate::random::Ids;
use groups::{ChannelRec, GroupRec};
use timeline::Timeline;

/// One of the provider's own users.
#[derive(Debug)]
pub(crate) struct User {
    pub(crate) actor: Actor,
    pub(crate) password: String,
    pub(crate) display_name: Option<String>,
    pub(crate) avatar: Option<String>,
    pub(crate) bio: Option<String>,
    pub(crate) updated_at: Timestamp,
    /// What the user set (§6.4); `online`/`offline` otherwise follow
    /// whether they have a live socket.
    pub(crate) availability: Availability,
    pub(crate) status: Option<String>,
    pub(crate) last_seen: Option<Timestamp>,
    pub(crate) follows: Vec<Follow>,
    /// Membership pointers to groups on other providers (§5.7), by URI.
    pub(crate) group_pointers: BTreeMap<String, GroupPointer>,
    pub(crate) dm_pointers: BTreeMap<DmId, DmPointer>,
    /// Read markers (§7.7): scope id → last read message.
    pub(crate) read_markers: BTreeMap<LocalId, LocalId>,
    pub(crate) notifications: Vec<Notification>,
}

/// A conversation pointer (§7.4).
#[derive(Debug, Clone)]
pub(crate) struct DmPointer {
    pub(crate) with: Actor,
    pub(crate) last_message: Option<Message>,
    /// When the provider stored `last_message`.
    pub(crate) stored_at: Option<Timestamp>,
    pub(crate) updated_at: Timestamp,
}

/// A registered device key (§4.3).
#[derive(Debug, Clone)]
pub(crate) struct DeviceKey {
    pub(crate) handle: String,
    pub(crate) key: VerifyingKey,
    pub(crate) public_key: String,
    pub(crate) device_name: String,
    pub(crate) created_at: Timestamp,
    pub(crate) revoked: bool,
}

/// A conversation this provider holds messages of: those whose recipient
/// is one of its users (§7.4, §8.3).
#[derive(Debug)]
pub(crate) struct DmRec {
    pub(crate) participants: [Actor; 2],
    pub(crate) timeline: Timeline,
}

impl DmRec {
    pub(crate) fn has(&self, actor: &Actor) -> bool {
        self.participants.contains(actor)
    }
}

/// A stored upload (§5.8).
#[derive(Debug, Clone)]
pub(crate) struct Upload {
    pub(crate) attachment: Attachment,
    pub(crate) uploader: Actor,
    pub(crate) target: Authority,
    pub(crate) bytes: Bytes,
}

#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) ids: Ids,
    pub(crate) users: BTreeMap<String, User>,
    pub(crate) keys: BTreeMap<String, DeviceKey>,
    /// Bootstrap tokens (§4.2): token → (handle, expiry).
    pub(crate) bootstrap: HashMap<String, (String, Timestamp)>,
    pub(crate) groups: BTreeMap<LocalId, GroupRec>,
    pub(crate) channels: BTreeMap<LocalId, ChannelRec>,
    pub(crate) dms: BTreeMap<DmId, DmRec>,
    pub(crate) uploads: HashMap<LocalId, Upload>,
    /// Capability URL token → upload id (§5.8 *Serving attachments*).
    pub(crate) served: HashMap<String, LocalId>,
}

impl State {
    /// The local user `actor` names, if it is one of this provider's.
    pub(crate) fn local_user(&self, authority: &Authority, actor: &Actor) -> Option<&User> {
        actor
            .is_local_to(authority)
            .then(|| self.users.get(actor.handle()))
            .flatten()
    }

    pub(crate) fn local_user_mut(
        &mut self,
        authority: &Authority,
        actor: &Actor,
    ) -> Option<&mut User> {
        if !actor.is_local_to(authority) {
            return None;
        }
        self.users.get_mut(actor.handle())
    }

    /// The group and channel a `(groupId, channelId)` pair names.
    pub(crate) fn channel(
        &self,
        group: &LocalId,
        channel: &LocalId,
    ) -> Option<(&GroupRec, &ChannelRec)> {
        let c = self.channels.get(channel)?;
        (c.channel.group_id == *group).then_some(())?;
        Some((self.groups.get(group)?, c))
    }

    /// The local user a conversation is with, for a counterparty `other`
    /// (§7.4 *Sending*: found by deriving the id for each local user).
    pub(crate) fn local_participant(
        &self,
        authority: &Authority,
        dm: &DmId,
        other: &Actor,
    ) -> Option<Actor> {
        self.users
            .values()
            .map(|u| &u.actor)
            .find(|u| *u != other && DmId::derive(u, other) == *dm)
            .filter(|u| u.is_local_to(authority))
            .cloned()
    }
}
