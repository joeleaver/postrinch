//! A fake provider, a transport that trusts it, and a signed-in user.

use std::sync::Arc;
use std::time::Duration;

use ofscp_core::LocalId;
use postrinch_ofscp::http::Client;
use postrinch_ofscp::{Endpoint, Options, RetryPolicy, Tls, Transport};
use postrinch_ofscp_fake::{FakeProvider, RecordedRequest, root_certificate_der};

pub const PASSWORD: &str = "a-test-password-0123";

/// A transport that trusts the fakes, with quick retries.
pub fn transport() -> Transport {
    let options = Options {
        retry: RetryPolicy {
            first_backoff: Duration::from_millis(20),
            ..RetryPolicy::default()
        },
        ..Options::default()
    };
    Transport::new(
        Tls::with_extra_roots(vec![root_certificate_der().to_vec()]).unwrap(),
        options,
    )
    .unwrap()
}

/// `handle` on `provider`, signed in over REST with a new device key.
pub async fn signed_in(transport: &Transport, provider: &FakeProvider, handle: &str) -> Client {
    provider.add_user(handle, PASSWORD);
    let endpoint = Endpoint::new(provider.authority());
    let identity = transport
        .provider(endpoint.clone())
        .sign_in(handle, PASSWORD, "integration test")
        .await
        .expect("sign-in");
    transport.client(Arc::new(identity), endpoint)
}

/// A provider with `alice` signed in, and her group and channel.
pub struct World {
    pub provider: FakeProvider,
    pub alice: Client,
    pub group: LocalId,
    pub channel: LocalId,
}

pub async fn world() -> World {
    let provider = FakeProvider::start().await.unwrap();
    let transport = transport();
    let alice = signed_in(&transport, &provider, "alice").await;
    let group = provider.create_group(alice.identity().actor(), "Group");
    let channel = provider.create_channel(&group, "general");
    World {
        provider,
        alice,
        group,
        channel,
    }
}

/// The recorded requests to `path`, oldest first.
pub fn requests_to(provider: &FakeProvider, path: &str) -> Vec<RecordedRequest> {
    provider
        .requests()
        .into_iter()
        .filter(|r| r.path == path)
        .collect()
}
