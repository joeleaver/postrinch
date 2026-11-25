//! A server too busy to answer: for one folder (`NO [INUSE] Mailbox is
//! locked`), which must not stop the other folders or hold removals back
//! for ever, or for the whole account (Gmail's bandwidth limit), which
//! stops the pass to be waited out. And `[LIMIT]`, which after signing in
//! is a refusal, not the server busy.

use std::sync::Arc;

use postrinch_mailtest::ImapFault;
use postrinch_sync::SyncError;

use super::sync_support::{fixture, mail};

/// Gmail's answer to any command once the account's daily limit is hit.
const BANDWIDTH: &str = "[ALERT] Account exceeded command or bandwidth limits. (Failure)";

#[tokio::test]
async fn a_folder_locked_on_every_pass_does_not_stop_the_others() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.user.append("INBOX", mail(Some("i@x"), "I", "")).unwrap();
    let a = f.user.append("Work", mail(Some("a@x"), "A", "")).unwrap();
    f.user.append("Work", mail(Some("b@x"), "B", "")).unwrap();
    // The inbox comes first in a pass, and is locked for good (Dovecot
    // with a stale lock).
    f.server.fail_next_matching(
        "SELECT",
        "\"INBOX\"",
        1000,
        ImapFault::No("[INUSE] Mailbox is locked".into()),
    );
    f.sync().await;
    assert!(f.message("a@x").is_some() && f.message("b@x").is_some());
    assert!(f.message("i@x").is_none(), "the inbox was not read");
    // Mail goes from Work: the unread inbox holds its removal back.
    f.user.expunge("Work", a).unwrap();
    f.sync().await;
    assert!(f.labels("a@x").is_empty(), "a@x left Work");
    assert!(f.message("a@x").is_some(), "held back by the unread inbox");
    // Work keeps syncing meanwhile.
    f.user.append("Work", mail(Some("c@x"), "C", "")).unwrap();
    f.sync().await;
    assert_eq!(f.labels("c@x").len(), 1);
    // After STUCK_AFTER (5) passes without it, the inbox stops holding
    // removals back.
    for _ in 0..3 {
        f.sync().await;
    }
    assert!(f.message("a@x").is_none(), "a@x is removed in the end");
    assert!(f.message("b@x").is_some() && f.message("c@x").is_some());
    assert!(f.label_names().contains(&"INBOX".to_string()));
}

#[tokio::test]
async fn a_folder_busy_for_a_fetch_counts_nothing_against_its_messages() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    // As many passes as a message may go unanswered (SILENT_TRIES, 3)
    // before it is kept as one that cannot be read.
    f.server.fail_next_matching(
        "UID FETCH",
        "BODY.PEEK[HEADER]",
        3,
        ImapFault::No("[UNAVAILABLE] Temporary System Error".into()),
    );
    for _ in 0..3 {
        f.sync().await;
        assert_eq!(f.message_count(), 0, "nothing stored, not even unreadable");
    }
    f.sync().await;
    assert_eq!(f.message("a@x").unwrap().subject(), "A");
}

#[tokio::test]
async fn limit_on_a_fetch_is_not_the_server_busy() {
    let f = fixture().await;
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.server.fail_next(
        "UID FETCH",
        1,
        ImapFault::No("[LIMIT] Too many UIDs in one command".into()),
    );
    let result = f.sync.sync(&f.me, Arc::new(|_| {})).await;
    assert!(result.is_ok(), "a refusal, not busy: {result:?}");
    f.sync().await;
    assert!(f.message("a@x").is_some());
}

#[tokio::test]
async fn an_account_wide_limit_stops_the_pass_and_loses_nothing() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.user.append("INBOX", mail(Some("a@x"), "A", "")).unwrap();
    f.user.append("Work", mail(Some("w@x"), "W", "")).unwrap();
    f.server
        .fail_next("UID FETCH", 1, ImapFault::No(BANDWIDTH.into()));
    let result = f.sync.sync(&f.me, Arc::new(|_| {})).await;
    assert!(
        matches!(result, Err(SyncError::Busy { .. })),
        "busy, not a partial answer: {result:?}"
    );
    let selected_work = |commands: &[String]| {
        commands
            .iter()
            .any(|c| c.starts_with("SELECT") && c.contains("\"Work\""))
    };
    assert!(!selected_work(&f.server.commands()), "the pass stopped");
    f.sync().await;
    assert!(f.message("a@x").is_some() && f.message("w@x").is_some());
}
