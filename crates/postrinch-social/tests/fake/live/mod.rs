//! Live sessions (the plan's PR 9): sockets to each provider, reconnects,
//! resumes and backfills, and what arrives on them.

mod dms;
mod dms_resume;
mod faults;
mod state;
mod timeline;

use std::sync::Arc;
use std::time::Duration;

use postrinch_ofscp::Endpoint;
use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::Actor;
use postrinch_social::{LiveEvent, LiveOptions, Social};
use tokio::sync::broadcast;

use crate::common::{Home, PASSWORD};

/// Quick reconnects, for tests.
pub fn options() -> LiveOptions {
    LiveOptions {
        first_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(200),
        ..LiveOptions::default()
    }
}

/// What a live engine told, read as it comes.
pub struct Told {
    events: broadcast::Receiver<LiveEvent>,
    /// Everything read so far, for failure messages and counting.
    pub seen: Vec<LiveEvent>,
}

impl Told {
    /// Read what `social` tells from now on.
    pub fn new(social: &Social) -> Self {
        Self {
            events: social.live_events(),
            seen: Vec::new(),
        }
    }

    /// Wait for the first event `wanted` matches, failing after a while.
    pub async fn until(&mut self, what: &str, wanted: impl Fn(&LiveEvent) -> bool) -> LiveEvent {
        let waiting = async {
            loop {
                let event = self.events.recv().await.expect("the engine is alive");
                self.seen.push(event.clone());
                if wanted(&event) {
                    return event;
                }
            }
        };
        match tokio::time::timeout(Duration::from_secs(15), waiting).await {
            Ok(event) => event,
            Err(_) => panic!("no {what}; told {:#?}", self.seen),
        }
    }

    /// Wait until every session of the account has caught up, `n` times
    /// in all from now.
    pub async fn caught_up(&mut self, n: usize) {
        for _ in 0..n {
            self.until("catching up", |e| matches!(e, LiveEvent::CaughtUp { .. }))
                .await;
        }
    }

    /// Read what else was told within `wait`.
    pub async fn settle(&mut self, wait: Duration) {
        let _ = tokio::time::timeout(wait, async {
            while let Ok(event) = self.events.recv().await {
                self.seen.push(event);
            }
        })
        .await;
    }

    /// How many `Message` events named `id`.
    pub fn messages_told(&self, id: &str) -> usize {
        self.seen
            .iter()
            .filter(|e| matches!(e, LiveEvent::Message { id: told, .. } if told == id))
            .count()
    }
}

/// Ada added, and her engine live; `sessions` caught up.
pub async fn live(h: &Home, sessions: usize) -> (Social, Told) {
    h.add().await;
    let social = h.social.clone().with_live_options(options());
    let mut told = Told::new(&social);
    social.go_live(&h.account()).await.expect("live");
    told.caught_up(sessions).await;
    (social, told)
}

/// `handle`, a new user of `h`'s home provider, signed in there.
pub async fn user(h: &Home, handle: &str) -> (Actor, Client) {
    let actor = h.provider.add_user(handle, PASSWORD);
    let endpoint = Endpoint::new(h.provider.authority());
    let identity = h
        .transport
        .provider(endpoint.clone())
        .sign_in(handle, PASSWORD, "theirs")
        .await
        .expect("they sign in");
    (actor, h.transport.client(Arc::new(identity), endpoint))
}

/// Whether `event` is a new message `id`.
pub fn is_new(event: &LiveEvent, id: &str) -> bool {
    matches!(
        event,
        LiveEvent::Message { id: told, change: postrinch_social::MessageChange::New, .. } if told == id
    )
}

/// Post `texts` in turn as `author`, each in a second of its own. Times
/// are written in whole seconds (as on the wire), and the store orders a
/// place's messages and moves read markers by when they were made, so
/// messages made in the same second are not told apart. Their ids, in
/// order.
pub async fn post_each(
    provider: &postrinch_ofscp_fake::FakeProvider,
    author: &Actor,
    group: &postrinch_ofscp::ofscp_core::LocalId,
    channel: &postrinch_ofscp::ofscp_core::LocalId,
    texts: &[&str],
) -> Vec<String> {
    let mut ids = Vec::new();
    for text in texts {
        let posted = provider.post_as(author, group, channel, text);
        while provider.now().as_second() == posted.created_at.as_second() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ids.push(posted.id.to_string());
    }
    ids
}

/// Run a request made from another of the person's devices (or another
/// person's). `Faults::drop_sockets` drops every connection, so one still
/// pooled on that device may be dead: it tries again, as a device would.
pub async fn elsewhere<T, F, Fut>(mut request: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = postrinch_ofscp::Result<T>>,
{
    for attempt in 1.. {
        match request().await {
            Ok(done) => return done,
            Err(postrinch_ofscp::Error::Network { .. }) if attempt < 3 => {}
            Err(error) => panic!("a request elsewhere failed: {error}"),
        }
    }
    unreachable!()
}
