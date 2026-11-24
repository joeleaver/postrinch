//! async-imap against the fake: sign-in, folders, fetching, changing mail.

use async_imap::types::{Flag, NameAttribute};
use futures_util::TryStreamExt;
use postrinch_mailtest::{ImapConfig, ImapFault, ImapServer, NewMessage};

use crate::helpers::{MAIL_1, MAIL_2, imap_client, imap_login};

async fn server() -> ImapServer {
    let server = ImapServer::start(
        ImapConfig::new()
            .user("bob", "hunter2")
            .oauth_token("bob", "tok"),
    )
    .await
    .unwrap();
    let bob = server.user("bob");
    bob.create_standard_folders().unwrap();
    bob.append("INBOX", NewMessage::new(MAIL_1)).unwrap();
    bob.append("INBOX", NewMessage::new(MAIL_2).flags(&["\\Seen"]))
        .unwrap();
    server
}

struct Sasl(Vec<u8>);

impl async_imap::Authenticator for &Sasl {
    type Response = Vec<u8>;
    fn process(&mut self, _challenge: &[u8]) -> Vec<u8> {
        self.0.clone()
    }
}

#[tokio::test]
async fn login_list_select_and_fetch() {
    let server = server().await;
    let mut session = imap_login(&server, "bob", "hunter2").await;

    let caps = session.capabilities().await.unwrap();
    for cap in [
        "IMAP4rev1",
        "CONDSTORE",
        "QRESYNC",
        "MOVE",
        "UIDPLUS",
        "SPECIAL-USE",
        "IDLE",
    ] {
        assert!(caps.has_str(cap), "missing {cap}");
    }

    let names: Vec<_> = session
        .list(Some(""), Some("*"))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let sent = names.iter().find(|n| n.name() == "Sent").unwrap();
    assert!(sent.attributes().contains(&NameAttribute::Sent));
    assert_eq!(sent.delimiter(), Some("/"));
    assert_eq!(names.len(), 6);

    let mailbox = session.select("INBOX").await.unwrap();
    let info = server.user("bob").folder("INBOX").unwrap();
    assert_eq!(mailbox.exists, 2);
    assert_eq!(mailbox.uid_validity, Some(info.uid_validity));
    assert_eq!(mailbox.uid_next, Some(3));
    assert_eq!(mailbox.highest_modseq, Some(info.highest_modseq));

    let fetches: Vec<_> = session
        .uid_fetch(
            "1:*",
            "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])",
        )
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(fetches.len(), 2);
    let first = &fetches[0];
    assert_eq!(first.uid, Some(1));
    assert_eq!(first.size, Some(MAIL_1.len() as u32));
    assert!(first.internal_date().is_some());
    assert!(first.header().unwrap().starts_with(b"From: Ann"));
    assert!(first.header().unwrap().ends_with(b"\r\n\r\n"));
    assert!(fetches[1].flags().any(|f| f == Flag::Seen));

    // BODY.PEEK leaves \Seen alone; BODY[] sets it.
    assert!(
        !server
            .user("bob")
            .message("INBOX", 1)
            .unwrap()
            .has_flag("\\Seen")
    );
    let bodies: Vec<_> = session
        .uid_fetch("1", "BODY[]")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(bodies[0].body(), Some(MAIL_1));
    assert!(bodies[0].flags().any(|f| f == Flag::Seen));
    assert!(
        server
            .user("bob")
            .message("INBOX", 1)
            .unwrap()
            .has_flag("\\Seen")
    );

    session.logout().await.unwrap();
    assert!(
        server
            .commands()
            .contains(&"LOGIN <credentials>".to_string())
    );
}

#[tokio::test]
async fn sign_in_failures_and_sasl() {
    let server = server().await;
    let refused = imap_client(&server).await.login("bob", "wrong").await;
    let (error, _) = refused.expect_err("a wrong password is refused");
    assert!(
        error.to_string().contains("AUTHENTICATIONFAILED"),
        "{error}"
    );

    let plain = Sasl(b"\0bob\0hunter2".to_vec());
    let session = imap_client(&server)
        .await
        .authenticate("PLAIN", &plain)
        .await;
    assert!(session.is_ok());

    let oauth = Sasl(b"user=bob\x01auth=Bearer tok\x01\x01".to_vec());
    let session = imap_client(&server)
        .await
        .authenticate("XOAUTH2", &oauth)
        .await;
    assert!(session.is_ok());

    let bad = Sasl(b"user=bob\x01auth=Bearer nope\x01\x01".to_vec());
    let refused = imap_client(&server)
        .await
        .authenticate("XOAUTH2", &bad)
        .await;
    assert!(refused.is_err());
    assert_eq!(server.command_count("AUTHENTICATE"), 3);
}

#[tokio::test]
async fn store_copy_move_expunge_search() {
    let server = server().await;
    let bob = server.user("bob");
    let mut session = imap_login(&server, "bob", "hunter2").await;
    session.select("INBOX").await.unwrap();

    let stored: Vec<_> = session
        .uid_store("1", "+FLAGS (\\Flagged)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(stored[0].flags().any(|f| f == Flag::Flagged));
    assert!(bob.message("INBOX", 1).unwrap().has_flag("\\Flagged"));

    let found = session.uid_search("SUBJECT invoice").await.unwrap();
    assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![2]);
    let found = session.uid_search("FLAGGED").await.unwrap();
    assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![1]);
    let found = session
        .uid_search("HEADER Message-ID <two@example.com>")
        .await
        .unwrap();
    assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![2]);
    let found = session
        .uid_search("SINCE 25-Nov-2000 TEXT noon")
        .await
        .unwrap();
    assert_eq!(found.into_iter().collect::<Vec<_>>(), vec![1]);

    session.uid_copy("1", "Archive").await.unwrap();
    assert_eq!(bob.messages("Archive").unwrap().len(), 1);
    assert!(bob.messages("Archive").unwrap()[0].has_flag("\\Flagged"));

    session.uid_mv("2", "Trash").await.unwrap();
    assert_eq!(bob.messages("Trash").unwrap()[0].body, MAIL_2);
    assert_eq!(bob.messages("INBOX").unwrap().len(), 1);

    session
        .uid_store("1", "+FLAGS.SILENT (\\Deleted)")
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let gone: Vec<_> = session
        .uid_expunge("1")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(gone.len(), 1);
    assert!(bob.messages("INBOX").unwrap().is_empty());
}

#[tokio::test]
async fn folders_and_status() {
    let server = server().await;
    let bob = server.user("bob");
    let mut session = imap_login(&server, "bob", "hunter2").await;

    session.create("Work/2026").await.unwrap();
    assert!(bob.folder("Work").is_ok());
    session.rename("Work", "Jobs").await.unwrap();
    assert!(bob.folder("Jobs/2026").is_ok());
    session.delete("Jobs/2026").await.unwrap();
    assert!(bob.folder("Jobs/2026").is_err());
    assert!(session.create("Jobs").await.is_err());

    session
        .append("Drafts", Some("(\\Draft \\Seen)"), None, MAIL_1)
        .await
        .unwrap();
    let drafts = bob.messages("Drafts").unwrap();
    assert!(drafts[0].has_flag("\\Draft") && drafts[0].has_flag("\\Seen"));

    let status = session
        .status(
            "INBOX",
            "(MESSAGES UIDNEXT UIDVALIDITY UNSEEN HIGHESTMODSEQ)",
        )
        .await
        .unwrap();
    assert_eq!(status.exists, 2);
    assert_eq!(status.uid_next, Some(3));
    assert_eq!(status.unseen, Some(1));
    assert!(status.highest_modseq.is_some());
}

#[tokio::test]
async fn programmed_failures() {
    let server = server().await;
    let bob = server.user("bob");
    let mut session = imap_login(&server, "bob", "hunter2").await;

    server.fail_next("SELECT", 1, ImapFault::No("[UNAVAILABLE] Try later".into()));
    let error = session.select("INBOX").await.unwrap_err();
    assert!(error.to_string().contains("UNAVAILABLE"), "{error}");
    session.select("INBOX").await.unwrap();

    // The store happens, but the client never hears so. (async-imap ends
    // a FETCH stream quietly when the connection drops, without the
    // tagged OK: a caller must not take an ended stream for success.)
    server.fail_next("UID STORE", 1, ImapFault::DisconnectAfter);
    let heard: Vec<_> = match session.uid_store("1", "+FLAGS (\\Flagged)").await {
        Ok(stream) => stream.try_collect().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    assert!(heard.is_empty());
    assert!(session.noop().await.is_err(), "the connection is gone");
    assert!(bob.message("INBOX", 1).unwrap().has_flag("\\Flagged"));
    assert_eq!(server.command_count("UID STORE"), 1);
}
