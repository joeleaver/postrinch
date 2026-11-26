//! What the keep-in-sync tests share: a recorder of what the loops
//! report, by kind, and waiting until it, or anything else, says what a
//! test wants.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use postrinch_sync::SyncEvent;

pub type Events = Arc<Mutex<Vec<SyncEvent>>>;

pub fn recorder() -> (Events, postrinch_sync::SyncEventFn) {
    let events: Events = Arc::default();
    let log = events.clone();
    (events, Arc::new(move |e| log.lock().unwrap().push(e)))
}

/// What happened, by kind, progress left out.
pub fn kinds(events: &Events) -> Vec<&'static str> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            SyncEvent::Started { .. } => Some("started"),
            SyncEvent::Progress { .. } => None,
            SyncEvent::Synced { .. } => Some("synced"),
            SyncEvent::Done { .. } => Some("done"),
            SyncEvent::Failed { .. } => Some("failed"),
            SyncEvent::NeedsSignIn { .. } => Some("needs sign-in"),
            SyncEvent::Busy { .. } => Some("busy"),
            SyncEvent::Sent { .. } => Some("sent"),
            SyncEvent::SendFailed { .. } => Some("send failed"),
        })
        .collect()
}

/// How many of the events are of this kind.
pub fn count(events: &Events, kind: &str) -> usize {
    kinds(events).iter().filter(|k| **k == kind).count()
}

pub fn busy_waits(events: &Events) -> Vec<(Duration, String)> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            SyncEvent::Busy {
                retry_in, message, ..
            } => Some((*retry_in, message.clone())),
            _ => None,
        })
        .collect()
}

/// How long the waits below give up after: only a hang ends one, so it is
/// long enough for the slowest machine the tests run on. Windows' CI
/// runner takes the whole suite about seven times as long as Linux's, and
/// a sync of fifty messages there, beside the other tests, took over 10 s.
const HUNG: Duration = Duration::from_secs(60);

/// Waits (up to [`HUNG`]) until `done` says the events are as wanted.
pub async fn until(events: &Events, done: impl Fn(&[&'static str]) -> bool) {
    let waited = tokio::time::timeout(HUNG, async {
        while !done(&kinds(events)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "gave up waiting: {:?}", kinds(events));
}

/// Waits (up to [`HUNG`]) until at least `passes` passes are done. At
/// least, not exactly: the waiting looks at the events only now and then,
/// and a pass that follows at once (one the last woke) may be done before
/// it looks again. A test that wants no more says so on its own.
pub async fn until_done(events: &Events, passes: usize) {
    until(events, |k| {
        k.iter().filter(|e| **e == "done").count() >= passes
    })
    .await;
}

/// Waits (up to [`HUNG`]) until `holds`, saying `what` if it never does.
pub async fn until_true(what: &str, holds: impl Fn() -> bool) {
    let waited = tokio::time::timeout(HUNG, async {
        while !holds() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "gave up waiting until {what}");
}
