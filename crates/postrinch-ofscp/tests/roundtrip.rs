//! Every sample in the spec's tests decodes into its type and encodes back
//! to the same JSON, so nothing a peer sends is lost or misnamed. The
//! samples the spec marks invalid are refused.

use std::collections::BTreeSet;

use postrinch_ofscp::sign::PublicKey;
use postrinch_ofscp::wire::*;
use postrinch_ofscp::ws::{Body, Command, Envelope, Event};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ofscp-v0.1");

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("{DIR}/{name}")).unwrap()
}

fn json(name: &str) -> Value {
    serde_json::from_str(&read(name)).unwrap()
}

fn roundtrip<T: Serialize + DeserializeOwned>(name: &str) {
    let decoded: T = serde_json::from_str(&read(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        serde_json::to_value(&decoded).unwrap(),
        json(name),
        "{name}"
    );
}

fn frame<T: Body + std::fmt::Debug>(name: &str, kinds: &[&str]) {
    let decoded = Envelope::<T>::decode(&read(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let kind = json(name)["type"].as_str().unwrap().to_owned();
    assert!(
        kinds.contains(&kind.as_str()),
        "{name}: {kind} decoded as Other"
    );
    let back: Value = serde_json::from_str(&decoded.encode().unwrap()).unwrap();
    assert_eq!(back, json(name), "{name}");
}

/// The REST samples, each with its type.
macro_rules! rest_samples {
    ($($file:literal => $ty:ty,)+) => {
        const REST: &[&str] = &[$($file),+];

        #[test]
        fn rest_samples_round_trip() {
            $( roundtrip::<$ty>($file); )+
        }
    };
}

rest_samples! {
    "attachment.sample.json" => Attachment,
    "auth-bootstrap-response.sample.json" => BootstrapResponse,
    "auth-login-request.sample.json" => LoginRequest,
    "auth-registration-request.sample.json" => RegistrationRequest,
    "channel-create-request.sample.json" => ChannelCreateRequest,
    "channel-update-request.sample.json" => ChannelUpdateRequest,
    "channel.sample.json" => Channel,
    "contact-create-request.sample.json" => ContactCreateRequest,
    "contact-event.sample.json" => ContactEvent,
    "contact.sample.json" => Contact,
    "contacts-response.sample.json" => ContactsResponse,
    "device-key-registration.sample.json" => DeviceKeyRegistration,
    "device-key-response.sample.json" => DeviceKeyResponse,
    "device-keys-response.sample.json" => DeviceKeysResponse,
    "discover-response.sample.json" => DiscoverResponse,
    "dm-conversation-pointer-request.sample.json" => ConversationPointer,
    "dm-conversation.sample.json" => DmConversation,
    "dm-conversations-response.sample.json" => DmConversationsResponse,
    "dm-message-create-request.sample.json" => DmMessageCreate,
    "follow-create-request.sample.json" => FollowCreateRequest,
    "follow.sample.json" => Follow,
    "follows-response.sample.json" => FollowsResponse,
    "group-create-request.sample.json" => GroupCreateRequest,
    "group-update-request.sample.json" => GroupUpdateRequest,
    "group.sample.json" => Group,
    "guest-claim-request.sample.json" => GuestClaimRequest,
    "guest-create-request.sample.json" => GuestCreateRequest,
    "guest-upgrade-response.sample.json" => GuestUpgradeResponse,
    "invite-create-request.sample.json" => InviteCreateRequest,
    "invite.sample.json" => Invite,
    "join-request.sample.json" => JoinRequest,
    "member.sample.json" => Member,
    "message-create-request.sample.json" => MessageCreate,
    "message-update-request.sample.json" => MessageUpdate,
    "message.sample.json" => Message,
    "messages-page.sample.json" => MessagesPage,
    "notification-setting.sample.json" => NotificationSetting,
    "notification-settings-response.sample.json" => NotificationSettingsResponse,
    "notification.sample.json" => Notification,
    "notifications-mark-response.sample.json" => NotificationsMarkResponse,
    "notifications-response.sample.json" => NotificationsResponse,
    "presence-update-request.sample.json" => PresenceUpdate,
    "presence.sample.json" => Presence,
    "privacy-settings-update-request.sample.json" => PrivacySettingsUpdate,
    "privacy-settings.sample.json" => PrivacySettings,
    "problem-details.sample.json" => ProblemDetails,
    "provider-discovery.sample.json" => ProviderDiscovery,
    "providers-response.sample.json" => ProvidersResponse,
    "reaction.sample.json" => Reaction,
    "read-markers-response.sample.json" => ReadMarkersResponse,
    "read-markers-update-request.sample.json" => ReadMarkersUpdateRequest,
    "tiers-response.sample.json" => TiersResponse,
    "user-keys-response.sample.json" => UserKeysResponse,
    "user-profile.sample.json" => UserProfile,
}

/// The WebSocket samples a client sends.
const COMMANDS: &[&str] = &[
    "ws-authenticate.sample.json",
    "ws-message-create.sample.json",
    "ws-message-delete.sample.json",
    "ws-message-update.sample.json",
    "ws-ping.sample.json",
    "ws-pong.sample.json",
    "ws-presence-set.sample.json",
    "ws-presence-subscribe.sample.json",
    "ws-presence-unsubscribe.sample.json",
    "ws-reaction-add.sample.json",
    "ws-reaction-remove.sample.json",
    "ws-subscribe.sample.json",
    "ws-typing-start.sample.json",
    "ws-typing-stop.sample.json",
    "ws-unsubscribe.sample.json",
];

/// The WebSocket samples a provider sends.
const EVENTS: &[&str] = &[
    "ws-auth-challenge.sample.json",
    "ws-authenticated.sample.json",
    "ws-channel-typing.sample.json",
    "ws-dm-conversation.sample.json",
    "ws-dm-message.sample.json",
    "ws-dm-reaction.sample.json",
    "ws-dm-typing.sample.json",
    "ws-error.sample.json",
    "ws-member-updated.sample.json",
    "ws-message-created.sample.json",
    "ws-message-deleted.sample.json",
    "ws-message-updated.sample.json",
    "ws-notification-created.sample.json",
    "ws-presence-subscribed.sample.json",
    "ws-presence-unsubscribed.sample.json",
    "ws-presence-update.sample.json",
    "ws-reaction-added.sample.json",
    "ws-reaction-removed.sample.json",
    "ws-read-updated.sample.json",
    "ws-subscribed.sample.json",
    "ws-unsubscribed-revoked.sample.json",
    "ws-unsubscribed.sample.json",
];

/// The rest: the spec's negative samples and the signing vectors
/// (tests/golden.rs).
const OTHERS: &[&str] = &[
    "device-key-registration.spki.invalid.json",
    "presence-update-request.offline.invalid.json",
    "signing-vector.json",
    "ws-signing-vector.json",
    "SOURCE.md",
];

#[test]
fn command_samples_round_trip() {
    for name in COMMANDS {
        frame::<Command>(name, Command::KINDS);
    }
}

#[test]
fn event_samples_round_trip() {
    for name in EVENTS {
        frame::<Event>(name, Event::KINDS);
    }
}

#[test]
fn setting_yourself_offline_is_refused() {
    let text = read("presence-update-request.offline.invalid.json");
    assert!(serde_json::from_str::<PresenceUpdate>(&text).is_err());
}

#[test]
fn a_der_public_key_is_unusable() {
    let reg: DeviceKeyRegistration =
        serde_json::from_str(&read("device-key-registration.spki.invalid.json")).unwrap();
    assert_eq!(PublicKey::from_base64(&reg.public_key), None);
    let good: DeviceKeyRegistration =
        serde_json::from_str(&read("device-key-registration.sample.json")).unwrap();
    assert!(PublicKey::from_base64(&good.public_key).is_some());
}

#[test]
fn every_fixture_is_tested() {
    let on_disk: BTreeSet<String> = std::fs::read_dir(DIR)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    let listed: BTreeSet<String> = [REST, COMMANDS, EVENTS, OTHERS]
        .concat()
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(on_disk, listed);
}
