//! The test-facing handle of one fake provider: start it, seed it, switch
//! its faults, inspect what it received.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use ofscp_core::signing::{VerifyingKey, encode_public_key};
use ofscp_core::wire::{
    Availability, Channel, ChannelType, Content, Group, Invite, JoinPolicy, JoinRequest,
    JoinRequestState, Member, Message, MessageCreate, NotificationSetting, Tier,
};
use ofscp_core::{Actor, Authority, LocalId};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::faults::Faults;
use crate::inner::Inner;
use crate::ops::messages;
use crate::random::token;
use crate::record::RecordedRequest;
use crate::state::groups::{ChannelRec, GroupRec, OWNER, effective_permissions};
use crate::state::timeline::Timeline;
use crate::state::{DeviceKey, User};

/// One fake OFSCP provider on a loopback port, serving HTTPS and the
/// WebSocket with the crate's test certificate. Dropping it shuts it down.
pub struct FakeProvider {
    inner: Arc<Inner>,
    task: JoinHandle<()>,
}

impl Drop for FakeProvider {
    fn drop(&mut self) {
        self.task.abort();
        self.inner.conns.abort_all();
        // Upgraded WebSockets have left the HTTP registry; end them too.
        self.inner.hub.shut_down();
    }
}

impl FakeProvider {
    /// Start a provider on `127.0.0.1` and a free port. Its authority is
    /// `127.0.0.1:PORT`. Needs a running Tokio runtime.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let authority = Authority::canonicalize(&format!("127.0.0.1:{port}"))
            .expect("an IP literal and a port form an authority");
        let inner = Arc::new(Inner::new(authority));
        let router = crate::http::router(inner.clone());
        let task = tokio::spawn(crate::server::serve(listener, router, inner.conns.clone()));
        Ok(Self { inner, task })
    }

    /// `127.0.0.1:PORT`, as actors and canonical strings carry it.
    pub fn authority(&self) -> Authority {
        self.inner.authority.clone()
    }

    /// `https://127.0.0.1:PORT`.
    pub fn base_url(&self) -> String {
        self.inner.base_url()
    }

    /// `wss://127.0.0.1:PORT/api/ws`.
    pub fn ws_url(&self) -> String {
        format!("wss://{}/api/ws", self.inner.authority.as_str())
    }

    /// The faults, switchable while the provider runs.
    pub fn faults(&self) -> Faults {
        Faults {
            inner: self.inner.clone(),
        }
    }

    /// Every HTTP request received so far, oldest first.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.inner.log.lock().unwrap().clone()
    }

    /// The actor `handle@authority` of this provider.
    ///
    /// # Panics
    /// If `handle` is not a valid handle (§2.1).
    pub fn actor(&self, handle: &str) -> Actor {
        Actor::parse_canonical(&format!("{handle}@{}", self.inner.authority.as_str()))
            .unwrap_or_else(|_| panic!("{handle:?} is not a valid handle"))
    }

    /// Create a user who can log in with `password` (§4.1.2).
    pub fn add_user(&self, handle: &str, password: &str) -> Actor {
        let actor = self.actor(handle);
        let now = self.inner.now();
        let user = User {
            actor: actor.clone(),
            password: password.to_owned(),
            display_name: None,
            avatar: None,
            bio: None,
            updated_at: now,
            availability: Availability::Online,
            status: None,
            last_seen: None,
            follows: Vec::new(),
            group_pointers: BTreeMap::new(),
            dm_pointers: BTreeMap::new(),
            read_markers: BTreeMap::new(),
            notifications: Vec::new(),
            notification_settings: Vec::new(),
        };
        self.inner
            .state
            .lock()
            .unwrap()
            .users
            .insert(handle.to_owned(), user);
        actor
    }

    /// Register a device key for a local user without the bootstrap
    /// exchange; returns its `key_id`.
    ///
    /// # Panics
    /// If `actor` is not a user of this provider.
    pub fn add_device_key(&self, actor: &Actor, key: VerifyingKey, device_name: &str) -> String {
        let mut state = self.inner.state.lock().unwrap();
        assert!(
            state.local_user(&self.inner.authority, actor).is_some(),
            "{actor} is not a user of this provider"
        );
        let key_id = format!("dk_{}", token(12));
        state.keys.insert(
            key_id.clone(),
            DeviceKey {
                handle: actor.handle().to_owned(),
                key,
                public_key: encode_public_key(&key),
                device_name: device_name.to_owned(),
                created_at: self.inner.now(),
                revoked: false,
            },
        );
        key_id
    }

    /// Create a group owned by `owner` (who may be a foreign actor): tier
    /// `group`, join policy `open`. [`configure_group`](Self::configure_group)
    /// changes either.
    pub fn create_group(&self, owner: &Actor, name: &str) -> LocalId {
        let now = self.inner.now();
        let mut state = self.inner.state.lock().unwrap();
        let id = state.ids.next("grp_");
        let group = Group {
            id: id.clone(),
            name: name.to_owned(),
            description: None,
            owner: owner.clone(),
            join_policy: JoinPolicy::Open,
            tier: Tier::Group,
            permissions: effective_permissions(None),
            roles: None,
            membership: None,
            created_at: now,
            updated_at: now,
            metadata: Vec::new(),
        };
        let mut members = BTreeMap::new();
        members.insert(owner.clone(), member(owner, OWNER, now));
        state.groups.insert(
            id.clone(),
            GroupRec {
                group,
                members,
                requests: Vec::new(),
            },
        );
        id
    }

    /// Change a seeded group as served (tier, join policy, permissions…).
    ///
    /// # Panics
    /// If there is no such group.
    pub fn configure_group(&self, group: &LocalId, change: impl FnOnce(&mut Group)) {
        let mut state = self.inner.state.lock().unwrap();
        let g = state.groups.get_mut(group).expect("no such group");
        change(&mut g.group);
        let given = std::mem::take(&mut g.group.permissions);
        g.group.permissions = effective_permissions(Some(given));
    }

    /// Delete a seeded group and its channels, as its owner deleting it
    /// would: afterwards it answers `404`.
    ///
    /// # Panics
    /// If there is no such group.
    pub fn remove_group(&self, group: &LocalId) {
        let mut state = self.inner.state.lock().unwrap();
        state.groups.remove(group).expect("no such group");
        state.channels.retain(|_, c| &c.channel.group_id != group);
    }

    /// Create a text channel, tier `group`, in `group`.
    ///
    /// # Panics
    /// If there is no such group.
    pub fn create_channel(&self, group: &LocalId, name: &str) -> LocalId {
        let now = self.inner.now();
        let mut state = self.inner.state.lock().unwrap();
        assert!(state.groups.contains_key(group), "no such group");
        let id = state.ids.next("chn_");
        let channel = Channel {
            id: id.clone(),
            group_id: group.clone(),
            name: Some(name.to_owned()),
            kind: ChannelType::Text,
            tier: Tier::Group,
            topic: None,
            tags: None,
            permissions: None,
            call: None,
            created_at: now,
            updated_at: now,
            metadata: Vec::new(),
        };
        let timeline = Timeline::new(id.as_str());
        state
            .channels
            .insert(id.clone(), ChannelRec { channel, timeline });
        id
    }

    /// Change a seeded channel as served (tier, permissions…).
    ///
    /// # Panics
    /// If there is no such channel.
    pub fn configure_channel(&self, channel: &LocalId, change: impl FnOnce(&mut Channel)) {
        let mut state = self.inner.state.lock().unwrap();
        change(
            &mut state
                .channels
                .get_mut(channel)
                .expect("no such channel")
                .channel,
        );
    }

    /// Make `actor` (local or foreign) a member of `group` with `role`.
    /// Their pending join requests count as approved.
    ///
    /// # Panics
    /// If there is no such group.
    pub fn add_member(&self, group: &LocalId, actor: &Actor, role: &str) {
        let now = self.inner.now();
        let mut state = self.inner.state.lock().unwrap();
        let g = state.groups.get_mut(group).expect("no such group");
        g.members.insert(actor.clone(), member(actor, role, now));
        for request in g.requests.iter_mut().filter(|r| r.user == *actor) {
            if request.state == JoinRequestState::Pending {
                request.state = JoinRequestState::Approved;
            }
        }
    }

    /// `group`'s members' roles, by actor: what the provider holds.
    ///
    /// # Panics
    /// If there is no such group.
    pub fn roles(&self, group: &LocalId) -> BTreeMap<Actor, String> {
        let state = self.inner.state.lock().unwrap();
        let g = state.groups.get(group).expect("no such group");
        g.members
            .iter()
            .map(|(actor, m)| (actor.clone(), m.role.clone()))
            .collect()
    }

    /// Every join request `group` has had, in the order made, with its
    /// state.
    ///
    /// # Panics
    /// If there is no such group.
    pub fn join_requests(&self, group: &LocalId) -> Vec<JoinRequest> {
        let state = self.inner.state.lock().unwrap();
        state
            .groups
            .get(group)
            .expect("no such group")
            .requests
            .clone()
    }

    /// The invites the provider holds for `group`, as it lists them (no
    /// tokens), with their uses.
    pub fn invites(&self, group: &LocalId) -> Vec<Invite> {
        let state = self.inner.state.lock().unwrap();
        state
            .invites
            .values()
            .filter(|r| r.invite.group_id == *group)
            .map(|r| r.invite.clone())
            .collect()
    }

    /// Post a chat message as `author`, under every rule a
    /// `message.create` meets; it is stored and fanned out to subscribers.
    ///
    /// # Panics
    /// If the rules refuse it (the author cannot post there, say).
    pub fn post_as(
        &self,
        author: &Actor,
        group: &LocalId,
        channel: &LocalId,
        text: &str,
    ) -> Message {
        let body = MessageCreate {
            client_message_id: None,
            kind: None,
            content: Content {
                text: text.to_owned(),
                mime: "text/plain".to_owned(),
            },
            attachments: None,
            reference: None,
            tags: None,
            mentions: None,
            title: None,
            summary: None,
            cover: None,
            metadata: None,
        };
        messages::create(&self.inner, author, group, channel, body, None)
            .unwrap_or_else(|p| panic!("the post was refused: {p:?}"))
            .into_inner()
    }

    /// The messages a channel holds, oldest first, tombstones included.
    pub fn messages(&self, group: &LocalId, channel: &LocalId) -> Vec<Message> {
        let state = self.inner.state.lock().unwrap();
        state
            .channel(group, channel)
            .map(|(_, c)| c.timeline.messages().map(|s| s.message.clone()).collect())
            .unwrap_or_default()
    }

    /// The provider an upload was made for (its `target`, §5.8), by the
    /// attachment id it was given; `None` for an id never uploaded here.
    pub fn upload_target(&self, attachment: &LocalId) -> Option<Authority> {
        let state = self.inner.state.lock().unwrap();
        state.uploads.get(attachment).map(|u| u.target.clone())
    }

    /// The notification settings `GET /api/me/notification-settings`
    /// gives `actor` (§10.1); none until set.
    ///
    /// # Panics
    /// If `actor` is not a user of this provider.
    pub fn set_notification_settings(&self, actor: &Actor, prefs: Vec<NotificationSetting>) {
        let mut state = self.inner.state.lock().unwrap();
        state
            .local_user_mut(&self.inner.authority, actor)
            .expect("no such user")
            .notification_settings = prefs;
    }

    /// The `cache_until` the keys endpoint gives (§4.6), from now.
    pub fn set_key_cache_ttl(&self, ttl: Duration) {
        self.inner.settings.lock().unwrap().key_cache_ttl = signed(ttl);
    }

    /// The advertised and enforced upload limit (§5.8).
    pub fn set_max_upload_bytes(&self, bytes: u64) {
        self.inner.settings.lock().unwrap().max_upload_bytes = bytes;
    }

    /// How long after creation chat lines and memos may be edited.
    pub fn set_edit_window(&self, window: Duration) {
        self.inner.settings.lock().unwrap().edit_window = signed(window);
    }

    /// The per-connection subscription cap (§7.1 *Limits*).
    pub fn set_subscription_cap(&self, cap: usize) {
        self.inner.settings.lock().unwrap().subscription_cap = cap;
    }

    /// Now on this provider's clock (offset by any skew fault).
    pub fn now(&self) -> Timestamp {
        self.inner.now()
    }

    /// How many WebSocket connections are signed in.
    pub fn signed_in_sockets(&self) -> usize {
        self.inner.hub.signed_in()
    }
}

fn member(actor: &Actor, role: &str, now: Timestamp) -> Member {
    Member {
        user: actor.clone(),
        role: role.to_owned(),
        display_name_override: None,
        joined_at: now,
    }
}

fn signed(d: Duration) -> SignedDuration {
    SignedDuration::try_from(d).expect("a test duration fits a SignedDuration")
}
