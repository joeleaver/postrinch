//! The IDLE's harder cases (docs/imap.md, "Sync"): mail that comes
//! while the IDLE is being made again, another connection refused for
//! the account's connection limit while the IDLE holds a place, a server
//! that ends each IDLE at once, and signing out of an IDLE that is let
//! go.

use std::time::{Duration, Instant};

use postrinch_mailtest::ImapFault;

use super::idle_support::{IDLE, KEEP, arrived, idling, keeping, mail_sync, sign_ins};
use super::keep_support::{count, kinds, until, until_done, until_true};
use super::support::USER;
use super::sync_support::{fixture, mail};

#[tokio::test]
async fn mail_that_comes_before_an_idle_made_again_begins_arrives_at_once() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    let events = keeping(&sync, KEEP, async |events, _| {
        until(events, |k| k.contains(&"done")).await;
        idling(&f.server, 1).await;
        // Delivered as the IDLE made again EXAMINEs the inbox, after the
        // pass the lost IDLE woke has read it: the EXAMINE counts it, and
        // no IDLE will tell of it.
        let user = f.server.user(USER);
        f.server.on_next("EXAMINE", "\"INBOX\"", move || {
            user.append("INBOX", mail(Some("gap@x"), "Gap", ""))
                .unwrap();
        });
        f.server.bye_idling("Server shutting down");
        // The poll is an hour away: only a pass the new IDLE wakes
        // brings it.
        until_true("the mail in the gap arrives", || arrived(&f, "gap@x")).await;
        idling(&f.server, 1).await;
    })
    .await;
    assert!(f.message("gap@x").is_some());
    assert!(!kinds(&events).contains(&"failed"), "{:?}", kinds(&events));
}

#[tokio::test]
async fn a_connection_refused_for_the_limit_makes_the_idle_give_way() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    let events = keeping(&sync, KEEP, async |events, wake| {
        until(events, |k| k.contains(&"done")).await;
        idling(&f.server, 1).await;
        // The sync connection and the IDLE's are signed in; from now on
        // the account may have one only. The sync connection is lost, so
        // the next pass must sign in again, and is refused.
        f.server.set_max_connections(1);
        f.server.fail_next("NOOP", 1, ImapFault::DisconnectBefore);
        wake.notify_one();
        until_done(events, 2).await;
        idling(&f.server, 0).await;
        // The IDLE that gave way wakes a pass, the inbox no longer
        // watched; it may follow the refused one at once.
        until_done(events, 3).await;
        f.user.append("INBOX", mail(Some("h@x"), "H", "")).unwrap();
        wake.notify_one();
        until_true("H arrives", || arrived(&f, "h@x")).await;
        tokio::time::sleep(IDLE.reconnect * 3).await;
        assert_eq!(f.server.idling(), 0, "it stays away");
    })
    .await;
    let kinds = kinds(&events);
    assert!(!kinds.contains(&"busy"), "no busy server shown: {kinds:?}");
    assert!(!kinds.contains(&"failed"), "{kinds:?}");
    // The sync connection, the IDLE's, the sync connection refused, and
    // again once the IDLE had signed out; the IDLE asks no more.
    assert_eq!(sign_ins(&f.server), 4);
    // It ended its IDLE before signing out: a LOGOUT sent mid-IDLE would
    // have been taken for a bad DONE, and neither logged.
    assert_eq!(f.server.command_count("DONE"), 1);
    let commands = f.server.commands();
    let ended = commands.iter().position(|c| c == "DONE").unwrap();
    assert!(
        commands[ended..].iter().any(|c| c == "LOGOUT"),
        "signed out after DONE: {commands:?}"
    );
}

#[tokio::test]
async fn a_connection_refused_while_the_idle_is_being_made_makes_it_give_way() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    // The IDLE connection signs in, then waits on its EXAMINE: signed in,
    // and counted by the server, but not yet watching.
    let examine = f.server.hold_next("EXAMINE", "\"INBOX\"");
    let events = keeping(&sync, KEEP, async |events, wake| {
        until(events, |k| k.contains(&"done")).await;
        until_true("the IDLE EXAMINEs the inbox", || {
            f.server.command_count("EXAMINE") == 1
        })
        .await;
        f.server.set_max_connections(1);
        f.server.fail_next("NOOP", 1, ImapFault::DisconnectBefore);
        wake.notify_one();
        // The sync connection, the IDLE's, and the sync connection
        // refused, while the IDLE's EXAMINE is unanswered.
        until_true("the pass is refused", || sign_ins(&f.server) == 3).await;
        examine.let_go();
        // Asked as it was being made, it signs out once made (or, should
        // its EXAMINE be answered before the pass reads its refusal, once
        // its IDLE has begun), and the pass signs in in its place. Which
        // comes first is the scheduler's; `imap::sync::idle_tests` holds
        // each step in turn.
        until_done(events, 2).await;
        f.user.append("INBOX", mail(Some("j@x"), "J", "")).unwrap();
        wake.notify_one();
        until_true("J arrives", || arrived(&f, "j@x")).await;
        tokio::time::sleep(IDLE.reconnect * 3).await;
        assert_eq!(f.server.idling(), 0, "it stays away");
    })
    .await;
    let kinds = kinds(&events);
    assert!(!kinds.contains(&"busy"), "no busy server shown: {kinds:?}");
    assert!(!kinds.contains(&"failed"), "{kinds:?}");
    assert_eq!(sign_ins(&f.server), 4);
    let idles = f.server.command_count("IDLE");
    assert_eq!(f.server.command_count("DONE"), idles, "any IDLE ended");
    assert_eq!(f.server.command_count("LOGOUT"), 1, "the IDLE's");
}

#[tokio::test]
async fn a_server_that_ends_each_idle_at_once_is_backed_off() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    // The first three IDLEs are answered OK at once, without a `+`.
    f.server
        .fail_next("IDLE", 3, ImapFault::Ok("IDLE completed".into()));
    let events = keeping(&sync, KEEP, async |events, _| {
        until(events, |k| k.contains(&"done")).await;
        let first = Instant::now();
        idling(&f.server, 1).await;
        // Each at least half its backoff: 100 ms, then 200, then 400.
        let waited = first.elapsed();
        let least = IDLE.reconnect / 2 + IDLE.reconnect + IDLE.reconnect * 2;
        assert!(waited >= least, "waited {waited:?}");
        assert_eq!(f.server.command_count("IDLE"), 4, "no IDLE sent in a loop");
        // The connection made again wakes a pass when its IDLE begins.
        until_done(events, 2).await;
        f.user.append("INBOX", mail(Some("i@x"), "I", "")).unwrap();
        until_true("I arrives", || arrived(&f, "i@x")).await;
    })
    .await;
    // Dropped each time, as a failed connection is.
    assert_eq!(sign_ins(&f.server), 5);
    assert!(!kinds(&events).contains(&"failed"), "{:?}", kinds(&events));
}

#[tokio::test]
async fn an_idle_let_go_is_ended_before_its_connection_signs_out() {
    let f = fixture().await;
    let sync = mail_sync(&f);
    let events = keeping(&sync, KEEP, async |events, _| {
        until(events, |k| k.contains(&"done")).await;
        idling(&f.server, 1).await;
        let logouts = f.server.command_count("LOGOUT");
        f.sync.disconnect(&f.me).await;
        idling(&f.server, 0).await;
        // The sync connection's LOGOUT, and the IDLE connection's, after
        // its DONE: a LOGOUT sent mid-IDLE would be taken for a bad DONE.
        until_true("both sign out", || {
            f.server.command_count("LOGOUT") == logouts + 2
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    })
    .await;
    assert_eq!(f.server.command_count("DONE"), 1);
    assert_eq!(count(&events, "done"), 1, "letting go wakes no pass");
}
