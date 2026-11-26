//! Keeping IMAP accounts in sync through [`MailSync`]: a server that is
//! busy is waited out with a growing wait, said with
//! [`SyncEvent::Busy`], and never taken for a refused sign-in; a pass
//! that signs in ends the wait, and so does adding the account again or
//! removing it; changes made here are sent when the loop is woken.

use std::sync::Arc;
use std::time::Duration;

use postrinch_mailtest::ImapFault;
use postrinch_model::MessageChange;
use postrinch_sync::{IdleOptions, KeepOptions, MailSync};
use tokio::sync::Notify;

use super::add::{Adding, adding};
use super::changes_support::record;
use super::keep_support::{busy_waits, kinds, recorder, until, until_done};
use super::support::{PASSWORD, USER, account};
use super::sync_support::{Fixture, fixture, mail};

/// Gmail's answer to any command once the account's daily limit is hit.
const BANDWIDTH: &str = "[ALERT] Account exceeded command or bandwidth limits. (Failure)";

const FAST: KeepOptions = KeepOptions {
    interval: Duration::from_secs(3600),
    retry: Duration::from_millis(50),
    idle: IdleOptions::DEFAULT,
};

const FIRST: Duration = Duration::from_millis(100);

/// Whether a wait is one of the first step's: between half of [`FIRST`]
/// and the whole of it.
fn first_step(wait: Duration) -> bool {
    wait >= FIRST / 2 && wait < FIRST
}

/// The façade over the fixture's IMAP account alone (no Google sign-in
/// in this copy), waiting out a busy server from [`FIRST`].
fn mail_sync(f: &Fixture) -> MailSync {
    MailSync::new(f.store.clone(), None, f.sync.clone()).with_busy_backoff(FIRST, FIRST * 8)
}

#[tokio::test]
async fn a_busy_server_is_waited_out_longer_each_time_and_never_needs_sign_in() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.server
        .fail_next("UID FETCH", 2, ImapFault::No(BANDWIDTH.into()));
    let sync = mail_sync(&f);
    let (events, log) = recorder();
    let wake = Arc::new(Notify::new());
    let keeping = sync.keep_in_sync(FAST, log, wake.clone());
    let watched = async {
        until(&events, |k| k.contains(&"done")).await;
        assert_eq!(
            kinds(&events),
            [
                "started", "busy", "started", "busy", "started", "synced", "done"
            ]
        );
        let waits = busy_waits(&events);
        // Each wait is between half and the whole of its step (jitter):
        // the first step is FIRST, the next twice that.
        assert!(first_step(waits[0].0), "the backoff's first: {waits:?}");
        assert!(
            waits[1].0 >= FIRST && waits[1].0 < FIRST * 2,
            "it grows: {waits:?}"
        );
        assert!(waits[0].1.contains("bandwidth"), "in the server's words");
        // A pass that signed in ended the wait: the next busy answer
        // waits the first wait again.
        f.server
            .fail_next("UID FETCH", 1, ImapFault::No(BANDWIDTH.into()));
        f.user.append("INBOX", mail(Some("b@x"), "B", "")).unwrap();
        wake.notify_one();
        until_done(&events, 2).await;
        let again = busy_waits(&events)[2].0;
        assert!(first_step(again), "the wait starts again: {again:?}");
    };
    tokio::select! {
        () = keeping => panic!("the loop ended"),
        () = watched => {}
    }
    assert!(!kinds(&events).contains(&"needs sign-in"));
    let account = f.store.account(&f.me).unwrap().unwrap();
    assert!(account.is_active(), "still signed in: {:?}", account.state);
    assert!(f.message("b@x").is_some());
}

#[tokio::test]
async fn a_wake_does_not_cut_a_busy_wait_short() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.server
        .fail_next("UID FETCH", 1, ImapFault::No(BANDWIDTH.into()));
    let sync = MailSync::new(f.store.clone(), None, f.sync.clone())
        .with_busy_backoff(Duration::from_secs(3600), Duration::from_secs(3600));
    let (events, log) = recorder();
    let wake = Arc::new(Notify::new());
    let keeping = sync.keep_in_sync(FAST, log, wake.clone());
    let watched = async {
        until(&events, |k| k.contains(&"busy")).await;
        let logins = f.server.command_count("LOGIN");
        for _ in 0..3 {
            wake.notify_one();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(kinds(&events), ["started", "busy"]);
        assert_eq!(f.server.command_count("LOGIN"), logins, "nothing asked");
    };
    tokio::select! {
        () = keeping => panic!("the loop ended"),
        () = watched => {}
    }
}

#[tokio::test]
async fn a_change_made_here_is_sent_when_woken() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.sync().await;
    let sync = mail_sync(&f);
    let (events, log) = recorder();
    let wake = Arc::new(Notify::new());
    let sending = sync.keep_sending(FAST, log, wake.clone());
    let watched = async {
        // Nothing waits: the loop waits to be woken.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(kinds(&events).is_empty(), "{:?}", kinds(&events));
        record(&f, &["a@x"], MessageChange::starred(true));
        wake.notify_one();
        until(&events, |k| k.contains(&"sent")).await;
    };
    tokio::select! {
        () = sending => panic!("the loop ended"),
        () = watched => {}
    }
    assert!(f.user.messages("INBOX").unwrap()[0].has_flag("\\Flagged"));
    assert_eq!(sync.waiting(&f.me).await.unwrap(), 0);
}

#[tokio::test]
async fn a_busy_server_holds_changes_back_without_losing_them() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.sync().await;
    record(&f, &["a@x"], MessageChange::starred(true));
    f.server
        .fail_next("UID STORE", 1, ImapFault::No(BANDWIDTH.into()));
    let sync = mail_sync(&f);
    let (events, log) = recorder();
    let sending = sync.keep_sending(FAST, log, Arc::new(Notify::new()));
    let watched = until(&events, |k| k.contains(&"sent"));
    tokio::select! {
        () = sending => panic!("the loop ended"),
        () = watched => {}
    }
    let kinds = kinds(&events);
    assert_eq!(kinds, ["busy", "sent"], "waited out, then sent");
    assert!(f.user.messages("INBOX").unwrap()[0].has_flag("\\Flagged"));
}

/// An account added with both fakes, a message waiting, and its next
/// `UID FETCH` answered busy; the façade waits out a busy server for an
/// hour.
async fn busy_for_an_hour() -> (Adding, MailSync) {
    let a = adding().await;
    let sync = MailSync::new(a.store.clone(), None, a.sync.clone())
        .with_busy_backoff(Duration::from_secs(3600), Duration::from_secs(3600));
    a.add_over(sync.imap(), PASSWORD).await;
    a.fakes
        .imap
        .user(USER)
        .append("INBOX", mail(Some("a@x"), "A", ""))
        .unwrap();
    a.fakes
        .imap
        .fail_next("UID FETCH", 1, ImapFault::No(BANDWIDTH.into()));
    (a, sync)
}

#[tokio::test]
async fn adding_the_account_again_ends_a_busy_wait() {
    let (a, sync) = busy_for_an_hour().await;
    let (events, log) = recorder();
    let wake = Arc::new(Notify::new());
    let keeping = sync.keep_in_sync(FAST, log, wake.clone());
    let watched = async {
        until(&events, |k| k.contains(&"busy")).await;
        // Its servers checked again (as when they change): the wait goes.
        a.add_over(sync.imap(), PASSWORD).await;
        wake.notify_one();
        until(&events, |k| k.contains(&"done")).await;
    };
    tokio::select! {
        () = keeping => panic!("the loop ended"),
        () = watched => {}
    }
}

#[tokio::test]
async fn an_account_removed_and_added_again_does_not_wait_out_the_old_wait() {
    let (a, sync) = busy_for_an_hour().await;
    let (events, log) = recorder();
    let wake = Arc::new(Notify::new());
    let keeping = sync.keep_in_sync(FAST, log, wake.clone());
    let watched = async {
        until(&events, |k| k.contains(&"busy")).await;
        sync.imap()
            .remove_account(&a.oauth, &account())
            .await
            .unwrap();
        wake.notify_one();
        a.add_over(sync.imap(), PASSWORD).await;
        wake.notify_one();
        until(&events, |k| k.contains(&"done")).await;
    };
    tokio::select! {
        () = keeping => panic!("the loop ended"),
        () = watched => {}
    }
}
