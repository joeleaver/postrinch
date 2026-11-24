//! RFC 7162 details: CLOSED, MODSEQ and UID in untagged FETCH, and
//! HIGHESTMODSEQ on EXPUNGE.

use postrinch_mailtest::{Extension, ImapConfig, ImapServer, NewMessage};

use crate::helpers::{MAIL_1, MAIL_2, Raw};

async fn server(config: ImapConfig) -> ImapServer {
    let server = ImapServer::start(config.user("bob", "hunter2"))
        .await
        .unwrap();
    let bob = server.user("bob");
    bob.create_folder("Archive").unwrap();
    bob.append("INBOX", NewMessage::new(MAIL_1)).unwrap();
    bob.append("INBOX", NewMessage::new(MAIL_2)).unwrap();
    server
}

const CLOSED: &str = "* OK [CLOSED] Previous mailbox closed";

#[tokio::test]
async fn closed_marks_every_implicit_close_when_condstore_is_offered() {
    for config in [
        ImapConfig::new(),
        ImapConfig::new().without(Extension::Qresync),
        ImapConfig::new().without(Extension::Condstore),
    ] {
        let server = server(config).await;
        let mut raw = Raw::login(&server, "bob", "hunter2").await;
        // Nothing was selected before: no CLOSED.
        let lines = raw.command("SELECT INBOX").await;
        assert!(!lines.contains(&CLOSED.to_string()), "{lines:?}");
        // No CONDSTORE enabling command was given, and still CLOSED comes
        // first, before anything about the new folder.
        let lines = raw.command("EXAMINE Archive").await;
        assert_eq!(lines[0], CLOSED, "{lines:?}");
        // A failed SELECT closes the old folder too.
        let lines = raw.command("SELECT Nowhere").await;
        assert_eq!(lines[0], CLOSED, "{lines:?}");
        assert!(lines[1].contains("NO [NONEXISTENT]"), "{lines:?}");
        // CLOSE and UNSELECT need no CLOSED, and leave nothing to close.
        raw.command("SELECT INBOX").await;
        let lines = raw.command("UNSELECT").await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        let lines = raw.command("SELECT INBOX").await;
        assert!(!lines.contains(&CLOSED.to_string()), "{lines:?}");
    }

    let server = server(
        ImapConfig::new()
            .without(Extension::Condstore)
            .without(Extension::Qresync),
    )
    .await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX").await;
    let lines = raw.command("SELECT Archive").await;
    assert!(!lines.contains(&CLOSED.to_string()), "{lines:?}");
}

#[tokio::test]
async fn condstore_only_store_and_seen_carry_modseq_but_uid_only_for_uid_commands() {
    let server = server(ImapConfig::new().without(Extension::Qresync)).await;
    let bob = server.user("bob");
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX (CONDSTORE)").await;

    let lines = raw.command("STORE 1 +FLAGS (\\Flagged)").await;
    let modseq = bob.message("INBOX", 1).unwrap().modseq;
    assert_eq!(
        lines[0],
        format!("* 1 FETCH (FLAGS (\\Flagged) MODSEQ ({modseq}))")
    );
    let lines = raw.command("UID STORE 1 -FLAGS (\\Flagged)").await;
    let modseq = bob.message("INBOX", 1).unwrap().modseq;
    assert_eq!(
        lines[0],
        format!("* 1 FETCH (UID 1 FLAGS () MODSEQ ({modseq}))")
    );

    // Reading the body sets \Seen: a new MODSEQ, reported with the flags,
    // and the folder's HIGHESTMODSEQ moves on.
    let before = bob.folder("INBOX").unwrap().highest_modseq;
    let lines = raw.command("FETCH 2 (BODY[TEXT])").await;
    let message = bob.message("INBOX", 2).unwrap();
    assert!(message.has_flag("\\Seen"));
    assert!(message.modseq > before);
    assert_eq!(bob.folder("INBOX").unwrap().highest_modseq, message.modseq);
    assert!(lines[0].starts_with("* 2 FETCH (BODY[TEXT] "), "{lines:?}");
    assert!(
        lines[0].ends_with(&format!(" FLAGS (\\Seen) MODSEQ ({}))", message.modseq)),
        "{lines:?}"
    );
    // A second read changes nothing and says nothing of flags.
    let lines = raw.command("FETCH 2 (BODY[TEXT])").await;
    assert!(!lines[0].contains("FLAGS"), "{lines:?}");
}

#[tokio::test]
async fn without_condstore_on_the_session_there_is_no_modseq() {
    let server = server(ImapConfig::new()).await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX").await;
    let lines = raw.command("STORE 1 +FLAGS (\\Flagged)").await;
    assert_eq!(lines[0], "* 1 FETCH (FLAGS (\\Flagged))");
    let lines = raw
        .command("FETCH 2 (BODY.PEEK[HEADER.FIELDS (Subject)] BODY[TEXT])")
        .await;
    assert!(lines[0].ends_with(" FLAGS (\\Seen))"), "{lines:?}");
    assert!(!lines[0].contains("UID"), "{lines:?}");
}

#[tokio::test]
async fn qresync_server_adds_the_uid_once_condstore_is_on() {
    let server = server(ImapConfig::new()).await;
    let bob = server.user("bob");
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX (CONDSTORE)").await;

    let lines = raw.command("STORE 1 +FLAGS (\\Flagged)").await;
    let modseq = bob.message("INBOX", 1).unwrap().modseq;
    assert_eq!(
        lines[0],
        format!("* 1 FETCH (UID 1 FLAGS (\\Flagged) MODSEQ ({modseq}))")
    );
    let lines = raw.command("FETCH 2 (BODY[TEXT])").await;
    let modseq = bob.message("INBOX", 2).unwrap().modseq;
    assert!(
        lines[0].starts_with("* 2 FETCH (UID 2 BODY[TEXT] "),
        "{lines:?}"
    );
    assert!(
        lines[0].ends_with(&format!(" FLAGS (\\Seen) MODSEQ ({modseq}))")),
        "{lines:?}"
    );
    let lines = raw.command("FETCH 1 (MODSEQ)").await;
    assert!(lines[0].starts_with("* 1 FETCH (UID 1 MODSEQ"), "{lines:?}");

    // A conditional .SILENT store that succeeds answers with MODSEQ even
    // when the flag was already set.
    let since = bob.folder("INBOX").unwrap().highest_modseq;
    let lines = raw
        .command(&format!(
            "STORE 1 (UNCHANGEDSINCE {since}) +FLAGS.SILENT (\\Flagged)"
        ))
        .await;
    let modseq = bob.message("INBOX", 1).unwrap().modseq;
    assert_eq!(lines[0], format!("* 1 FETCH (UID 1 MODSEQ ({modseq}))"));
}

#[tokio::test]
async fn expunge_reports_highestmodseq_under_qresync() {
    let server = server(ImapConfig::new()).await;
    let bob = server.user("bob");
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("ENABLE QRESYNC").await;
    raw.command("SELECT INBOX").await;

    // An expunge that removes nothing still reports HIGHESTMODSEQ.
    let lines = raw.command("EXPUNGE").await;
    let highest = bob.folder("INBOX").unwrap().highest_modseq;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines
            .last()
            .unwrap()
            .ends_with(&format!(" OK [HIGHESTMODSEQ {highest}] EXPUNGE completed")),
        "{lines:?}"
    );

    raw.command("STORE 1 +FLAGS.SILENT (\\Deleted)").await;
    let lines = raw.command("EXPUNGE").await;
    let highest = bob.folder("INBOX").unwrap().highest_modseq;
    assert_eq!(lines[0], "* VANISHED 1");
    assert!(
        lines
            .last()
            .unwrap()
            .ends_with(&format!(" OK [HIGHESTMODSEQ {highest}] EXPUNGE completed")),
        "{lines:?}"
    );

    raw.command("UID STORE 2 +FLAGS.SILENT (\\Deleted)").await;
    let lines = raw.command("UID EXPUNGE 2").await;
    let highest = bob.folder("INBOX").unwrap().highest_modseq;
    assert_eq!(lines[0], "* VANISHED 2");
    assert!(
        lines
            .last()
            .unwrap()
            .ends_with(&format!(" OK [HIGHESTMODSEQ {highest}] EXPUNGE completed")),
        "{lines:?}"
    );

    // Without QRESYNC enabled, a plain OK.
    bob.append("INBOX", NewMessage::new(MAIL_1).flags(&["\\Deleted"]))
        .unwrap();
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX (CONDSTORE)").await;
    let lines = raw.command("EXPUNGE").await;
    assert_eq!(lines[0], "* 1 EXPUNGE");
    assert!(
        lines.last().unwrap().ends_with(" OK EXPUNGE completed"),
        "{lines:?}"
    );
}
