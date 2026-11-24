//! What sync relies on: CONDSTORE, QRESYNC, IDLE, UIDVALIDITY, STARTTLS.

use std::time::Duration;

use async_imap::extensions::idle::IdleResponse;
use futures_util::TryStreamExt;
use postrinch_mailtest::{Extension, ImapConfig, ImapServer, NewMessage};
use tokio::net::TcpStream;

use crate::helpers::{MAIL_1, MAIL_2, Raw, imap_login, tls_over};

async fn server(config: ImapConfig) -> ImapServer {
    let server = ImapServer::start(config.user("bob", "hunter2"))
        .await
        .unwrap();
    let bob = server.user("bob");
    bob.append("INBOX", NewMessage::new(MAIL_1)).unwrap();
    bob.append("INBOX", NewMessage::new(MAIL_2)).unwrap();
    server
}

#[tokio::test]
async fn condstore_changedsince() {
    let server = server(ImapConfig::new()).await;
    let bob = server.user("bob");
    let mut session = imap_login(&server, "bob", "hunter2").await;
    let mailbox = session.select_condstore("INBOX").await.unwrap();
    let since = mailbox.highest_modseq.unwrap();

    bob.add_flags("INBOX", 2, &["\\Seen"]).unwrap();
    let changed: Vec<_> = session
        .uid_fetch("1:*", format!("(FLAGS) (CHANGEDSINCE {since})"))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    // The flag change may also arrive as an unsolicited FETCH first; what
    // matters is the answer names only message 2, with its new MODSEQ.
    let uids: Vec<_> = changed.iter().filter_map(|f| f.uid).collect();
    assert!(uids.iter().all(|&u| u == 2) && !uids.is_empty(), "{uids:?}");
    assert_eq!(
        changed.last().unwrap().modseq,
        Some(bob.message("INBOX", 2).unwrap().modseq)
    );
    assert!(
        server
            .commands()
            .iter()
            .any(|c| c.contains(&format!("(CHANGEDSINCE {since})")))
    );
}

#[tokio::test]
async fn idle_hears_new_mail_and_expunges() {
    let server = server(ImapConfig::new()).await;
    let bob = server.user("bob");
    let mut session = imap_login(&server, "bob", "hunter2").await;
    session.select("INBOX").await.unwrap();

    let mut idle = session.idle();
    idle.init().await.unwrap();
    let (wait, _stop) = idle.wait_with_timeout(Duration::from_secs(10));
    let pushing = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        bob.append("INBOX", NewMessage::new(MAIL_1)).unwrap();
    };
    let (response, ()) = tokio::join!(wait, pushing);
    match response.unwrap() {
        IdleResponse::NewData(data) => {
            let text = format!("{:?}", data.parsed());
            assert!(text.contains("Exists(3)"), "{text}");
        }
        other => panic!("expected new data, got {other:?}"),
    }
    let session = idle.done().await.unwrap();
    drop(session);
    assert_eq!(server.command_count("IDLE"), 1);
}

#[tokio::test]
async fn raw_idle_reports_expunge_and_flags_with_sequence_numbers() {
    let server = server(ImapConfig::new()).await;
    let bob = server.user("bob");
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX").await;
    raw.send(b"i1 IDLE\r\n").await;
    assert_eq!(raw.line().await, "+ idling");

    bob.expunge("INBOX", 1).unwrap();
    assert_eq!(raw.line().await, "* 1 EXPUNGE");
    bob.add_flags("INBOX", 2, &["\\Flagged"]).unwrap();
    assert_eq!(raw.line().await, "* 1 FETCH (UID 2 FLAGS (\\Flagged))");

    raw.send(b"DONE\r\n").await;
    assert_eq!(raw.line().await, "i1 OK IDLE terminated");
}

#[tokio::test]
async fn qresync_select_and_vanished() {
    let server = server(ImapConfig::new()).await;
    let bob = server.user("bob");
    let info = bob.folder("INBOX").unwrap();
    bob.append("INBOX", NewMessage::new(MAIL_1)).unwrap();
    bob.expunge("INBOX", 1).unwrap();
    bob.add_flags("INBOX", 2, &["\\Seen"]).unwrap();

    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    let enabled = raw.command("ENABLE QRESYNC").await;
    assert_eq!(enabled[0], "* ENABLED QRESYNC");
    let lines = raw
        .command(&format!(
            "SELECT INBOX (QRESYNC ({} {}))",
            info.uid_validity, info.highest_modseq
        ))
        .await;
    assert!(
        lines.contains(&"* VANISHED (EARLIER) 1".to_string()),
        "{lines:?}"
    );
    let modseq2 = bob.message("INBOX", 2).unwrap().modseq;
    let modseq3 = bob.message("INBOX", 3).unwrap().modseq;
    assert!(lines.contains(&format!(
        "* 1 FETCH (UID 2 FLAGS (\\Seen) MODSEQ ({modseq2}))"
    )));
    assert!(lines.contains(&format!("* 2 FETCH (UID 3 FLAGS () MODSEQ ({modseq3}))")));
    assert!(lines.last().unwrap().contains("[READ-WRITE]"));

    let lines = raw
        .command(&format!(
            "UID FETCH 1:* (FLAGS) (CHANGEDSINCE {} VANISHED)",
            info.highest_modseq
        ))
        .await;
    assert_eq!(lines[0], "* VANISHED (EARLIER) 1");
    assert_eq!(lines.len(), 4, "{lines:?}");

    // An expunge seen while selected is VANISHED, not EXPUNGE.
    bob.expunge("INBOX", 2).unwrap();
    let lines = raw.command("NOOP").await;
    assert_eq!(lines[0], "* VANISHED 2");
}

#[tokio::test]
async fn uidvalidity_change_ends_the_session() {
    let server = server(ImapConfig::new()).await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX").await;
    server.user("bob").set_uid_validity("INBOX", 9).unwrap();
    let lines_before_close = {
        raw.send(b"t9 NOOP\r\n").await;
        raw.line().await
    };
    assert_eq!(lines_before_close, "* BYE The selected mailbox is gone");
    assert!(raw.closed().await);

    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    let lines = raw.command("SELECT INBOX").await;
    assert!(lines.contains(&"* OK [UIDVALIDITY 9] UIDs valid".to_string()));
    assert!(lines.contains(&"* OK [UIDNEXT 3] Predicted next UID".to_string()));
}

#[tokio::test]
async fn starttls_then_login_with_async_imap() {
    let server = server(ImapConfig::new().implicit_tls(false).starttls(true)).await;
    assert!(server.tls_addr().is_none());
    let tcp = TcpStream::connect(server.starttls_addr().unwrap())
        .await
        .unwrap();
    let mut client = async_imap::Client::new(tcp);
    client.read_response().await.unwrap().unwrap();
    client
        .run_command_and_check_ok("STARTTLS", None)
        .await
        .unwrap();
    let tls = tls_over(client.into_inner(), server.certificate()).await;
    // After STARTTLS the server does not greet again.
    let client = async_imap::Client::new(tls);
    let mut session = client
        .login("bob", "hunter2")
        .await
        .map_err(|(e, _)| e)
        .unwrap();
    assert_eq!(session.select("INBOX").await.unwrap().exists, 2);
}

#[tokio::test]
async fn extensions_can_be_withheld() {
    let config = ImapConfig::new().extensions(&[Extension::Idle]);
    let server = server(config).await;
    let mut session = imap_login(&server, "bob", "hunter2").await;
    let caps = session.capabilities().await.unwrap();
    assert!(caps.has_str("IDLE") && !caps.has_str("CONDSTORE") && !caps.has_str("MOVE"));
    let mailbox = session.select("INBOX").await.unwrap();
    assert_eq!(mailbox.highest_modseq, None);
    assert!(session.uid_mv("1", "INBOX").await.is_err());

    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX").await;
    let lines = raw.command("UID COPY 1 INBOX").await;
    assert_eq!(lines.last().unwrap(), "t3 OK COPY completed");
    let lines = raw.command("UID EXPUNGE 1").await;
    assert!(lines.last().unwrap().starts_with("t4 BAD"));
}

#[tokio::test]
async fn disconnect_all_drops_idle_sessions() {
    let server = server(ImapConfig::new()).await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX").await;
    raw.send(b"i1 IDLE\r\n").await;
    assert_eq!(raw.line().await, "+ idling");
    server.disconnect_all();
    assert!(raw.closed().await);
    // New connections are served as before.
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    assert!(raw.command("NOOP").await[0].contains("OK"));
}
