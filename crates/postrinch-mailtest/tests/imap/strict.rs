//! Where the fake is as strict as the RFCs: LIST-EXTENDED only when
//! offered, keywords without regard to case, oversized literals.

use postrinch_mailtest::{Extension, ImapConfig, ImapServer, NewMessage, SpecialUse};

use crate::helpers::{MAIL_1, MAIL_2, Raw};

async fn server(config: ImapConfig) -> ImapServer {
    let server = ImapServer::start(config.user("bob", "hunter2"))
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

fn is_bad(lines: &[String]) -> bool {
    lines.len() == 1 && lines[0].split(' ').nth(1) == Some("BAD")
}

#[tokio::test]
async fn extended_list_forms_need_list_extended() {
    let server = server(ImapConfig::new().without(Extension::ListExtended)).await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    let caps = raw.command("CAPABILITY").await;
    assert!(!caps[0].contains("LIST-EXTENDED"), "{caps:?}");
    for command in [
        "LIST (SUBSCRIBED) \"\" *",
        "LIST () \"\" *",
        "LIST \"\" * RETURN (CHILDREN)",
        "LIST \"\" (INBOX Archive)",
        "LIST (SPECIAL-USE) \"\" * RETURN (SPECIAL-USE)",
    ] {
        let lines = raw.command(command).await;
        assert!(is_bad(&lines), "{command}: {lines:?}");
    }
    // A plain LIST still shows the special use (RFC 6154 section 2).
    let lines = raw.command("LIST \"\" Archive").await;
    assert_eq!(
        lines[0],
        "* LIST (\\HasNoChildren \\Archive) \"/\" \"Archive\""
    );
}

#[tokio::test]
async fn list_extended_selection_and_return() {
    let server = server(ImapConfig::new()).await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    let caps = raw.command("CAPABILITY").await;
    assert!(caps[0].contains(" LIST-EXTENDED"), "{caps:?}");

    let lines = raw
        .command("LIST \"\" (INBOX Archive) RETURN (SUBSCRIBED)")
        .await;
    assert_eq!(
        lines[..2],
        [
            "* LIST (\\HasNoChildren \\Archive \\Subscribed) \"/\" \"Archive\"",
            "* LIST (\\HasNoChildren \\Subscribed) \"/\" \"INBOX\"",
        ]
    );

    // RECURSIVEMATCH alone is BAD (RFC 5258 3.1).
    let lines = raw.command("LIST (RECURSIVEMATCH) \"\" %").await;
    assert!(is_bad(&lines), "{lines:?}");

    // A parent no longer subscribed, with a subscribed child, comes back
    // with CHILDINFO under RECURSIVEMATCH, and not at all without it.
    raw.command("UNSUBSCRIBE Lists").await;
    let lines = raw.command("LIST (SUBSCRIBED RECURSIVEMATCH) \"\" %").await;
    assert!(
        lines.contains(
            &"* LIST (\\Noselect \\HasChildren) \"/\" \"Lists\" (\"CHILDINFO\" (\"SUBSCRIBED\"))"
                .to_string()
        ),
        "{lines:?}"
    );
    let lines = raw.command("LIST (SUBSCRIBED) \"\" %").await;
    assert!(!lines.iter().any(|l| l.contains("\"Lists\"")), "{lines:?}");
}

#[tokio::test]
async fn keywords_compare_without_case() {
    let server = server(ImapConfig::new()).await;
    let bob = server.user("bob");
    let mut raw = Raw::login(&server, "bob", "hunter2").await;
    raw.command("SELECT INBOX").await;

    raw.command("STORE 1 +FLAGS ($Label)").await;
    // The same keyword in another case: stored as first seen.
    let lines = raw.command("STORE 2 +FLAGS ($LABEL)").await;
    assert_eq!(lines[0], "* 2 FETCH (FLAGS ($Label))");
    assert!(bob.message("INBOX", 2).unwrap().has_flag("$Label"));
    // Adding it again changes nothing.
    let lines = raw.command("STORE 1 +FLAGS.SILENT ($label)").await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(bob.message("INBOX", 1).unwrap().flags, ["$Label"]);

    let lines = raw.command("SEARCH KEYWORD $label").await;
    assert_eq!(lines[0], "* SEARCH 1 2");
    let lines = raw.command("STORE 1 -FLAGS ($LABEL)").await;
    assert_eq!(lines[0], "* 1 FETCH (FLAGS ())");
    let lines = raw.command("UID SEARCH UNKEYWORD $lAbEl").await;
    assert_eq!(lines[0], "* SEARCH 1");
    let lines = raw.command("SEARCH KEYWORD \\SEEN").await;
    assert_eq!(lines[0], "* SEARCH");
}

#[tokio::test]
async fn an_oversized_literal_is_refused_without_running_its_bytes() {
    let server = server(ImapConfig::new()).await;
    let mut raw = Raw::login(&server, "bob", "hunter2").await;

    // A synchronizing literal: refused before the client sends it.
    raw.send(b"a1 APPEND INBOX {70000000}\r\n").await;
    let line = raw.line().await;
    assert!(line.starts_with("a1 BAD [TOOBIG]"), "{line}");

    // A non-synchronizing one is on its way: the server reads past it,
    // and the commands it seems to hold are not run.
    let size = 64 * 1024 * 1024 + 1;
    let mut literal = b"x1 LOGOUT\r\n".repeat(size / 11 + 1);
    literal.truncate(size);
    raw.send(format!("a2 APPEND INBOX {{{size}+}}\r\n").as_bytes())
        .await;
    raw.send(&literal).await;
    raw.send(b"\r\n").await;
    let line = raw.line().await;
    assert!(line.starts_with("a2 BAD [TOOBIG]"), "{line}");
    let lines = raw.command("NOOP").await;
    assert!(lines.last().unwrap().contains(" OK "), "{lines:?}");
    assert_eq!(server.user("bob").folder("INBOX").unwrap().messages, 2);
    assert_eq!(server.command_count("LOGOUT"), 0);
}
