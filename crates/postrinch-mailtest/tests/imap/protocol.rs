//! Exact responses, checked line by line.

use postrinch_mailtest::{ImapConfig, ImapServer, NewMessage, SpecialUse};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::helpers::{MAIL_1, MAIL_2, Raw};

async fn server() -> ImapServer {
    let server = ImapServer::start(ImapConfig::new().user("bob", "hunter2"))
        .await
        .unwrap();
    let bob = server.user("bob");
    bob.create_special_folder("Archive", SpecialUse::Archive)
        .unwrap();
    bob.create_noselect_folder("Lists").unwrap();
    bob.create_folder("Lists/rust").unwrap();
    bob.append("INBOX", NewMessage::new(MAIL_1)).unwrap();
    bob.append("INBOX", NewMessage::new(MAIL_2)).unwrap();
    server
}

#[tokio::test]
async fn literals_synchronizing_and_non_synchronizing() {
    let server = server().await;
    let mut raw = Raw::connect(&server).await;
    raw.send(b"a1 LOGIN {3}\r\n").await;
    assert_eq!(raw.line().await, "+ Ready for literal data");
    raw.send(b"bob {7+}\r\nhunter2\r\n").await;
    assert!(raw.line().await.starts_with("a1 OK"));

    let message = b"Subject: (odd) \"quoted\" {5}\r\n\r\nline\r\n";
    raw.send(
        format!(
            "a2 APPEND Archive (\\Seen) \"24-Nov-2026 09:05:00 +0100\" {{{}}}\r\n",
            message.len()
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(raw.line().await, "+ Ready for literal data");
    raw.send(message).await;
    raw.send(b"\r\n").await;
    let reply = raw.line().await;
    let validity = server.user("bob").folder("Archive").unwrap().uid_validity;
    assert_eq!(
        reply,
        format!("a2 OK [APPENDUID {validity} 1] APPEND completed")
    );
    let stored = server.user("bob").message("Archive", 1).unwrap();
    assert_eq!(stored.body, message);
    assert_eq!(
        stored.internal_date.to_rfc3339(),
        "2026-11-24T09:05:00+01:00"
    );
    assert!(stored.has_flag("\\Seen"));
}

#[tokio::test]
async fn list_variants() {
    let server = server().await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    let lines = raw.command("LIST \"\" \"\"").await;
    assert_eq!(lines[0], "* LIST (\\Noselect) \"/\" \"\"");

    let lines = raw.command("LIST \"\" %").await;
    assert_eq!(
        lines[..3],
        [
            "* LIST (\\HasNoChildren \\Archive) \"/\" \"Archive\"",
            "* LIST (\\HasNoChildren) \"/\" \"INBOX\"",
            "* LIST (\\Noselect \\HasChildren) \"/\" \"Lists\"",
        ]
    );
    assert_eq!(lines.len(), 4);

    let lines = raw
        .command("LIST (SPECIAL-USE) \"\" * RETURN (SPECIAL-USE)")
        .await;
    assert_eq!(lines.len(), 2);
    let lines = raw.command("LIST \"Lists/\" *").await;
    assert_eq!(lines[0], "* LIST (\\HasNoChildren) \"/\" \"Lists/rust\"");

    raw.command("UNSUBSCRIBE Archive").await;
    let lines = raw.command("LSUB \"\" *").await;
    assert!(!lines.iter().any(|l| l.contains("Archive")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("INBOX")));
}

#[tokio::test]
async fn select_examine_fetch_sections() {
    let server = server().await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    let lines = raw.command("EXAMINE INBOX").await;
    assert!(lines.contains(&"* 2 EXISTS".to_string()));
    assert!(lines.contains(&"* OK [PERMANENTFLAGS ()] Flags permitted".to_string()));
    assert!(lines.last().unwrap().contains("[READ-ONLY]"));

    let lines = raw
        .command("FETCH 2 (BODY[HEADER.FIELDS (Subject)] BODY[]<0.4>)")
        .await;
    assert_eq!(
        lines[0],
        "* 2 FETCH (BODY[HEADER.FIELDS (SUBJECT)] {20}Subject: Invoice\r\n\r\n BODY[]<0> {4}From)"
    );
    // EXAMINE is read-only: BODY[] does not set \Seen, STORE is refused.
    assert!(
        !server
            .user("bob")
            .message("INBOX", 2)
            .unwrap()
            .has_flag("\\Seen")
    );
    let lines = raw.command("STORE 1 +FLAGS (\\Seen)").await;
    assert!(lines[0].contains("NO [READ-ONLY]"));

    let lines = raw.command("SELECT Lists").await;
    assert!(lines.last().unwrap().contains("NO"));
    let lines = raw.command("FETCH 1 FLAGS").await;
    assert!(
        lines.last().unwrap().contains("BAD"),
        "nothing is selected now"
    );
    let lines = raw.command("FETCH 1 ENVELOPE").await;
    assert!(lines.last().unwrap().contains("BAD"));
}

#[tokio::test]
async fn copy_move_and_expunge_sequence_numbers() {
    let server = server().await;
    let bob = server.user("bob");
    bob.append("INBOX", NewMessage::new(MAIL_1)).unwrap();
    let archive = bob.folder("Archive").unwrap().uid_validity;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX").await;

    let lines = raw.command("UID COPY 1:2 Archive").await;
    assert_eq!(
        lines[0],
        format!("t3 OK [COPYUID {archive} 1:2 1:2] COPY completed")
    );
    let lines = raw.command("UID COPY 1 Nowhere").await;
    assert!(lines[0].contains("NO [TRYCREATE]"));

    let lines = raw.command("UID MOVE 2:* Archive").await;
    assert_eq!(
        lines,
        [
            format!("* OK [COPYUID {archive} 2:3 3:4] Moved"),
            "* 3 EXPUNGE".to_string(),
            "* 2 EXPUNGE".to_string(),
            "t5 OK MOVE completed".to_string(),
        ]
    );

    bob.append("INBOX", NewMessage::new(MAIL_2)).unwrap();
    let lines = raw.command("STORE 1:* +FLAGS.SILENT (\\Deleted)").await;
    assert_eq!(lines, ["* 2 EXISTS", "t6 OK STORE completed"]);
    let lines = raw.command("EXPUNGE").await;
    assert_eq!(
        lines,
        ["* 2 EXPUNGE", "* 1 EXPUNGE", "t7 OK EXPUNGE completed"]
    );
}

#[tokio::test]
async fn conditional_store() {
    let server = server().await;
    let bob = server.user("bob");
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX (CONDSTORE)").await;
    let before = bob.folder("INBOX").unwrap().highest_modseq;
    bob.add_flags("INBOX", 2, &["$Label"]).unwrap();
    let lines = raw
        .command(&format!(
            "UID STORE 1:2 (UNCHANGEDSINCE {before}) +FLAGS.SILENT (\\Seen)"
        ))
        .await;
    let modseq = bob.message("INBOX", 1).unwrap().modseq;
    // The other client's change is reported first, then the store.
    assert!(
        lines[0].starts_with("* 2 FETCH (UID 2 FLAGS ($Label) MODSEQ"),
        "{lines:?}"
    );
    assert_eq!(lines[1], format!("* 1 FETCH (UID 1 MODSEQ ({modseq}))"));
    assert!(
        lines.last().unwrap().starts_with("t3 OK [MODIFIED 2]"),
        "{lines:?}"
    );
    assert!(!bob.message("INBOX", 2).unwrap().has_flag("\\Seen"));
}

#[tokio::test]
async fn bad_syntax_and_states() {
    let server = server().await;
    let mut raw = Raw::connect(&server).await;
    let lines = raw.command("SELECT INBOX").await;
    assert!(lines[0].contains("BAD"));
    raw.send(b"x1 LOGIN \"unterminated\r\n").await;
    assert!(raw.line().await.starts_with("x1 BAD"));
    raw.send(b"\r\n").await;
    assert!(raw.line().await.starts_with("* BAD"));
    let lines = raw.command("FROB").await;
    assert!(lines[0].starts_with("t2 BAD"));
    let lines = raw.command("LOGOUT").await;
    assert_eq!(lines[0], "* BYE Logging out");
    assert!(raw.closed().await);
}

#[tokio::test]
async fn plain_port_requires_starttls() {
    let server = ImapServer::start(ImapConfig::new().user("bob", "pw").starttls(true))
        .await
        .unwrap();
    let tcp = TcpStream::connect(server.starttls_addr().unwrap())
        .await
        .unwrap();
    let mut stream = BufReader::new(tcp);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert!(
        line.contains("STARTTLS") && line.contains("LOGINDISABLED"),
        "{line}"
    );
    stream
        .get_mut()
        .write_all(b"a LOGIN bob pw\r\n")
        .await
        .unwrap();
    line.clear();
    stream.read_line(&mut line).await.unwrap();
    assert!(line.starts_with("a NO [PRIVACYREQUIRED]"), "{line}");
}
