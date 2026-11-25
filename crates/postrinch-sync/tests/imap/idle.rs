//! IDLE on the inbox (docs/imap.md, "Sync"), through [`MailSync`]'s
//! keep-in-sync loop: news wakes a pass and the mail arrives; an IDLE is
//! begun again on the same connection when it is time; a lost connection
//! is made again after a growing wait; a busy server is waited out and
//! never asks the person to sign in; a server without IDLE is polled; a
//! removed account's IDLE goes, and comes back when it is added again;
//! and a connection limit makes the IDLE give way to the others.
//!
//! Every pass after the first comes from IDLE or from a wake the test
//! gives: the poll interval is an hour, unless a test polls.

use std::sync::Arc;
use std::time::{Duration, Instant};

use postrinch_mailtest::{Extension, ImapConfig, ImapFault, ImapServer};
use postrinch_model::FolderRoles;
use postrinch_sync::imap::message_id;
use postrinch_sync::{IdleOptions, KeepOptions, MailSync, SyncEvent};
use tokio::sync::Notify;

use super::add::adding;
use super::keep_support::{Events, count, kinds, recorder, until, until_true};
use super::support::{PASSWORD, USER, account};
use super::sync_support::{Fixture, fixture, fixture_with, fixture_without, mail};

const IDLE: IdleOptions = IdleOptions {
    restart: Duration::from_secs(3600),
    settle: Duration::from_millis(30),
    interval: Duration::from_secs(3600),
    give_way: Duration::from_secs(3600),
    reconnect: Duration::from_millis(100),
    reconnect_cap: Duration::from_millis(800),
};

const KEEP: KeepOptions = KeepOptions {
    interval: Duration::from_secs(3600),
    retry: Duration::from_millis(50),
    idle: IDLE,
};

/// The busy gate's first wait.
const FIRST: Duration = Duration::from_millis(100);

fn mail_sync(f: &Fixture) -> MailSync {
    MailSync::new(f.store.clone(), None, f.sync.clone()).with_busy_backoff(FIRST, FIRST * 8)
}

/// How many times the client signed in.
fn sign_ins(server: &ImapServer) -> usize {
    server.command_count("LOGIN") + server.command_count("AUTHENTICATE")
}

/// Runs the keep-in-sync loop with `options` while `test` runs.
async fn keeping(
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
fn arrived(f: &Fixture, header: &str) -> bool {
    matches!(f.store.message(&f.me, &f.id(header)), Ok(Some(_)))
}

/// Waits until the server has this many sessions in an IDLE.
async fn idling(server: &ImapServer, sessions: usize) {
    until_true(&format!("{sessions} IDLEing"), || {
        server.idling() == sessions
    })
    .await;
}

#[tokio::test]
async fn new_mail_wakes_a_pass_and_it_arrives() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    let events = keeping(&sync, KEEP, async |events, _| {
        until(events, |k| k.contains(&"done")).await;
        idling(&f.server, 1).await;
        // A burst: three messages at once wake one pass.
        for n in 0..3 {
            let id = format!("new{n}@x");
            f.user.append("INBOX", mail(Some(&id), "New", "")).unwrap();
        }
        until(events, |k| k.iter().filter(|e| **e == "done").count() == 2).await;
        for n in 0..3 {
            assert!(f.message(&format!("new{n}@x")).is_some(), "message {n}");
        }
        idling(&f.server, 1).await;
        tokio::time::sleep(IDLE.settle * 5).await;
        assert_eq!(count(events, "done"), 2, "one pass for the burst");
        // A flag set elsewhere, and a message gone, are news too.
        let uid = f.user.messages("INBOX").unwrap()[0].uid;
        f.user.add_flags("INBOX", uid, &["\\Flagged"]).unwrap();
        until(events, |k| k.iter().filter(|e| **e == "done").count() == 3).await;
        idling(&f.server, 1).await;
        f.user.expunge("INBOX", uid).unwrap();
        until(events, |k| k.iter().filter(|e| **e == "done").count() == 4).await;
    })
    .await;
    assert!(!kinds(&events).contains(&"failed"), "{:?}", kinds(&events));
    let commands = f.server.commands();
    assert!(
        commands.iter().any(|c| c.starts_with("EXAMINE \"INBOX\"")),
        "the inbox, read-only: {commands:?}"
    );
    // The sync connection, and the IDLE's: no more.
    assert_eq!(sign_ins(&f.server), 2);
}

#[tokio::test]
async fn an_idle_is_begun_again_on_the_same_connection_when_it_is_time() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    let options = KeepOptions {
        idle: IdleOptions {
            restart: Duration::from_millis(100),
            ..IDLE
        },
        ..KEEP
    };
    let events = keeping(&sync, options, async |events, _| {
        until(events, |k| k.contains(&"done")).await;
        until_true("four IDLEs", || f.server.command_count("IDLE") >= 4).await;
    })
    .await;
    assert_eq!(sign_ins(&f.server), 2, "the same connection each time");
    assert_eq!(count(&events, "done"), 1, "a restart wakes no pass");
}

#[tokio::test]
async fn a_lost_connection_is_made_again_after_a_growing_wait() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    let events = keeping(&sync, KEEP, async |events, _| {
        until(events, |k| k.contains(&"done")).await;
        idling(&f.server, 1).await;
        let idles = f.server.command_count("IDLE");
        // The next connection is dropped as soon as it IDLEs.
        f.server.fail_next("IDLE", 1, ImapFault::DisconnectBefore);
        let dropped = Instant::now();
        f.server.disconnect_all();
        // News may have been missed: the drop wakes a pass.
        until(events, |k| k.iter().filter(|e| **e == "done").count() == 2).await;
        idling(&f.server, 1).await;
        // The first wait is at least half of `reconnect`, the second at
        // least half of twice it.
        let waited = dropped.elapsed();
        assert!(
            waited >= IDLE.reconnect / 2 + IDLE.reconnect,
            "waited {waited:?}"
        );
        assert_eq!(f.server.command_count("IDLE"), idles + 2);
        // It works again: new mail arrives.
        f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
        until(events, |k| k.iter().filter(|e| **e == "done").count() == 3).await;
    })
    .await;
    assert!(f.message("b@x").is_some());
    // The IDLE that never began woke no pass of its own.
    assert_eq!(count(&events, "done"), 3);
}

#[tokio::test]
async fn a_busy_server_during_idle_is_waited_out_and_never_needs_sign_in() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    let events = keeping(&sync, KEEP, async |events, _| {
        until(events, |k| k.contains(&"done")).await;
        idling(&f.server, 1).await;
        f.server
            .bye_idling("[ALERT] Account exceeded command or bandwidth limits. (Failure)");
        until(events, |k| k.contains(&"busy")).await;
        // Waited out, it IDLEs again.
        idling(&f.server, 1).await;
        f.user.append("INBOX", mail(Some("c@x"), "C", "")).unwrap();
        until_true("C arrives", || arrived(&f, "c@x")).await;
    })
    .await;
    assert!(f.message("c@x").is_some());
    let busy = events.lock().unwrap().iter().find_map(|e| match e {
        SyncEvent::Busy { message, .. } => Some(message.clone()),
        _ => None,
    });
    assert!(
        busy.is_some_and(|m| m.contains("bandwidth")),
        "in the server's words"
    );
    assert!(!kinds(&events).contains(&"needs sign-in"));
    let stored = f.store.account(&f.me).unwrap().unwrap();
    assert!(stored.is_active(), "still signed in: {:?}", stored.state);
}

#[tokio::test]
async fn a_server_without_idle_is_polled() {
    let f = fixture_without(&[Extension::Idle]).await;
    let sync = mail_sync(&f);
    let polled = KeepOptions {
        interval: Duration::from_millis(100),
        ..KEEP
    };
    keeping(&sync, polled, async |events, _| {
        f.user.append("INBOX", mail(Some("d@x"), "D", "")).unwrap();
        until(events, |k| k.iter().filter(|e| **e == "done").count() >= 3).await;
    })
    .await;
    assert!(f.message("d@x").is_some());
    assert_eq!(f.server.command_count("IDLE"), 0);
    // Signed in once to learn that, and not again.
    assert_eq!(sign_ins(&f.server), 2);
}

#[tokio::test]
async fn too_many_connections_makes_idle_give_way_and_the_account_is_polled() {
    let f = fixture_with(ImapConfig::new().max_connections(1), FolderRoles::default()).await;
    let sync = mail_sync(&f);
    let polled = KeepOptions {
        interval: Duration::from_millis(100),
        ..KEEP
    };
    let events = keeping(&sync, polled, async |events, _| {
        until(events, |k| k.contains(&"done")).await;
        f.user.append("INBOX", mail(Some("e@x"), "E", "")).unwrap();
        until_true("E arrives", || arrived(&f, "e@x")).await;
        until(events, |k| k.iter().filter(|e| **e == "done").count() >= 4).await;
    })
    .await;
    assert!(f.message("e@x").is_some());
    assert_eq!(f.server.command_count("IDLE"), 0);
    // The sync connection's sign-in and the IDLE's one refused one: it
    // gives way for `give_way` and asks no more meanwhile.
    assert_eq!(sign_ins(&f.server), 2);
    let kinds = kinds(&events);
    assert!(
        !kinds.contains(&"busy"),
        "the account is not busy: {kinds:?}"
    );
    assert!(!kinds.contains(&"failed"), "{kinds:?}");
}

#[tokio::test]
async fn a_removed_account_stops_idling_and_idles_again_when_added_back() {
    let a = adding().await;
    let sync = MailSync::new(a.store.clone(), None, a.sync.clone());
    a.add_over(sync.imap(), PASSWORD).await;
    let server = &a.fakes.imap;
    keeping(&sync, KEEP, async |events, wake| {
        until(events, |k| k.contains(&"done")).await;
        idling(server, 1).await;
        sync.imap()
            .remove_account(&a.oauth, &account())
            .await
            .unwrap();
        idling(server, 0).await;
        wake.notify_one();
        tokio::time::sleep(IDLE.reconnect * 3).await;
        assert_eq!(server.idling(), 0, "gone with the account");
        a.add_over(sync.imap(), PASSWORD).await;
        wake.notify_one();
        idling(server, 1).await;
        server
            .user(USER)
            .append("INBOX", mail(Some("f@x"), "F", ""))
            .unwrap();
        until(events, |k| k.iter().filter(|e| **e == "done").count() == 3).await;
    })
    .await;
    let id = message_id(&account(), "f@x");
    let message = a.store.message(&account(), &id).unwrap();
    assert!(message.is_some(), "F arrived after the re-add");
}

#[tokio::test]
async fn signing_in_anew_idles_again_with_the_new_sign_in() {
    let a = adding().await;
    let sync = MailSync::new(a.store.clone(), None, a.sync.clone());
    a.add_over(sync.imap(), PASSWORD).await;
    let server = &a.fakes.imap;
    keeping(&sync, KEEP, async |events, wake| {
        until(events, |k| k.contains(&"done")).await;
        idling(server, 1).await;
        let before = sign_ins(server);
        // Added again over itself: its sign-in kept anew, and its
        // connections let go.
        a.add_over(sync.imap(), PASSWORD).await;
        idling(server, 0).await;
        wake.notify_one();
        idling(server, 1).await;
        // The pass, and the IDLE, signed in again.
        assert!(sign_ins(server) >= before + 2, "signed in anew");
        until(events, |k| k.iter().filter(|e| **e == "done").count() == 2).await;
    })
    .await;
}
