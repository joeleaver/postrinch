//! A fake home provider with Ada on it, signed in on her "phone" too (to
//! set things up as another device would), and a social engine.

use std::sync::Arc;
use std::time::Duration;

use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::wire::DeviceKeyInfo;
use postrinch_ofscp::ofscp_core::{Actor, LocalId};
use postrinch_ofscp::{Endpoint, Options, RetryPolicy, Tls, Transport};
use postrinch_ofscp_fake::{FakeProvider, RecordedRequest, root_certificate_der};
use postrinch_social::{Added, Social};
use postrinch_store::{SocialMessage, Store, social_account_key};
use postrinch_sync_core::MemorySecrets;

pub const PASSWORD: &str = "a-test-password-0123";
pub const DEVICE: &str = "Postrinch under test";

/// A transport that trusts the fakes, with quick retries.
pub fn transport() -> Transport {
    let options = Options {
        retry: RetryPolicy {
            first_backoff: Duration::from_millis(20),
            ..RetryPolicy::default()
        },
        ..Options::default()
    };
    let tls = Tls::with_extra_roots(vec![root_certificate_der().to_vec()]).unwrap();
    Transport::new(tls, options).unwrap()
}

pub struct Home {
    pub provider: FakeProvider,
    pub social: Social,
    pub keys: Arc<MemorySecrets>,
    pub ada: Actor,
    /// Ada on another device, with a key of its own.
    pub phone: Client,
    pub transport: Transport,
    _dir: tempfile::TempDir,
}

pub async fn home() -> Home {
    let provider = FakeProvider::start().await.unwrap();
    let ada = provider.add_user("ada", PASSWORD);
    let transport = transport();
    let endpoint = Endpoint::new(provider.authority());
    let phone = transport
        .provider(endpoint.clone())
        .sign_in("ada", PASSWORD, "phone")
        .await
        .expect("the phone signs in");
    let phone = transport.client(Arc::new(phone), endpoint);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let keys = Arc::new(MemorySecrets::default());
    let social = Social::new(store, transport.clone(), keys.clone()).with_device_name(DEVICE);
    Home {
        provider,
        social,
        keys,
        ada,
        phone,
        transport,
        _dir: dir,
    }
}

impl Home {
    /// Ada's actor, as the store keys her account.
    pub fn account(&self) -> String {
        self.ada.to_string()
    }

    /// Add Ada's account.
    pub async fn add(&self) -> Added {
        let found = self.social.look_up(&self.account()).await.expect("look-up");
        self.social.add(&found, PASSWORD).await.expect("add")
    }

    /// The keychain entry kept for Ada.
    pub fn key_entry(&self) -> Option<String> {
        self.keys
            .get_key(&social_account_key(&self.account()))
            .unwrap()
    }

    /// Ada's registered device keys, as her phone sees them.
    pub async fn device_keys(&self) -> Vec<DeviceKeyInfo> {
        self.phone
            .device_keys()
            .await
            .expect("the device keys")
            .keys
    }

    /// A user of the home provider, with a group Ada is in and a channel.
    pub fn bob_with_group(&self) -> (Actor, LocalId, LocalId) {
        let bob = self.provider.add_user("bob", PASSWORD);
        let group = self.provider.create_group(&bob, "Bob's group");
        let channel = self.provider.create_channel(&group, "general");
        self.provider.add_member(&group, &self.ada, "member");
        (bob, group, channel)
    }

    /// The messages kept for a place, oldest first.
    pub fn messages(&self, place: &str) -> Vec<SocialMessage> {
        let mut found = self
            .social
            .store()
            .social_place_messages(&self.account(), place)
            .unwrap();
        found.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        found
    }
}

/// `https://{provider}/api/groups/{group}/channels/{channel}`.
pub fn channel_uri(provider: &FakeProvider, group: &LocalId, channel: &LocalId) -> String {
    format!(
        "https://{}/api/groups/{group}/channels/{channel}",
        provider.authority()
    )
}

/// `https://{provider}/api/groups/{group}`.
pub fn group_uri(provider: &FakeProvider, group: &LocalId) -> String {
    format!("https://{}/api/groups/{group}", provider.authority())
}

/// The recorded requests to `path`, oldest first.
pub fn requests_to(provider: &FakeProvider, path: &str) -> Vec<RecordedRequest> {
    provider
        .requests()
        .into_iter()
        .filter(|r| r.path == path)
        .collect()
}
