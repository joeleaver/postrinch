//! What the IDLE tests share: options that leave every pass after the
//! first to IDLE or to the test (the poll interval is an hour, unless a
//! test polls), the keep-in-sync loop run beside a test, and waiting for
//! the fake server's IDLEs.

use std::sync::Arc;
use std::time::Duration;

use postrinch_mailtest::ImapServer;
use postrinch_sync::{IdleOptions, KeepOptions, MailSync};
use tokio::sync::Notify;

use super::keep_support::{Events, recorder, until_true};
use super::sync_support::Fixture;

pub const IDLE: IdleOptions = IdleOptions {
    restart: Duration::from_secs(3600),
    settle: Duration::from_millis(30),
    interval: Duration::from_secs(3600),
    give_way: Duration::from_secs(3600),
    reconnect: Duration::from_millis(100),
    reconnect_cap: Duration::from_millis(800),
};

pub const KEEP: KeepOptions = KeepOptions {
    interval: Duration::from_secs(3600),
    retry: Duration::from_millis(50),
    idle: IDLE,
};

/// The busy gate's first wait.
pub const FIRST: Duration = Duration::from_millis(100);

pub fn mail_sync(f: &Fixture) -> MailSync {
    MailSync::new(f.store.clone(), None, f.sync.clone()).with_busy_backoff(FIRST, FIRST * 8)
}

/// How many times the client signed in.
pub fn sign_ins(server: &ImapServer) -> usize {
    server.command_count("LOGIN") + server.command_count("AUTHENTICATE")
}

/// Runs the keep-in-sync loop with `options` while `test` runs.
pub async fn keeping(
    sync: &MailSync,
    options: KeepOptions,
    test: impl AsyncFnOnce(&Events, Arc<Notify>),
) -> Events {
    let (events, log) = recorder();
    let wake = Arc::new(Notify::new());
    tokio::select! {
        () = sync.keep_in_sync(options, log, wake.clone()) => panic!("the loop ended"),
        () = test(&events, wake) => {}
    }
    events
}

/// Whether the message has been stored. Polled while a pass may be
/// storing it, so a read that meets it half written counts as not yet;
/// each test checks it strictly once the loop has stopped.
pub fn arrived(f: &Fixture, header: &str) -> bool {
    matches!(f.store.message(&f.me, &f.id(header)), Ok(Some(_)))
}

/// Waits until the server has this many sessions in an IDLE.
pub async fn idling(server: &ImapServer, sessions: usize) {
    until_true(&format!("{sessions} IDLEing"), || {
        server.idling() == sessions
    })
    .await;
}
