//! Searching the server for mail not downloaded yet (docs/imap.md,
//! "Searching the server"): what is found is stored by its headers, newest
//! first, in the folders a search covers; a busy server is busy, never a
//! sign-in; a search the server refuses says why.

use chrono::{DateTime, FixedOffset};
use postrinch_mailtest::{ImapConfig, ImapFault, SpecialUse};
use postrinch_model::{FolderRoles, ImapAlternative, ImapSearch, SearchKey};
use postrinch_sync::SyncError;
use postrinch_sync::imap::ServerFound;

use super::sync_support::{Fixture, fixture, fixture_with, mail};

/// Gmail's answer to any command once the account's daily limit is hit.
const BANDWIDTH: &str = "[ALERT] Account exceeded command or bandwidth limits. (Failure)";

/// A search for `word` in the text, wherever a search covers.
pub(super) fn text(word: &str) -> ImapAlternative {
    ImapAlternative {
        folders: Vec::new(),
        not_in: Vec::new(),
        set_aside_too: false,
        key: SearchKey::Text(word.into()),
        gmail: word.into(),
        inexact: None,
    }
}

pub(super) fn one(alternative: ImapAlternative) -> ImapSearch {
    ImapSearch {
        alternatives: vec![alternative],
    }
}

pub(super) fn on(day: u32) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(&format!("2026-11-{day:02}T09:00:00+00:00")).unwrap()
}

impl Fixture {
    pub(super) async fn search(
        &self,
        search: &ImapSearch,
        max: usize,
    ) -> Result<ServerFound, SyncError> {
        self.sync.search_server(&self.me, search, max).await
    }

    /// How many commands the server has had: where to count from.
    fn mark(&self) -> usize {
        self.server.commands().len()
    }

    /// The folders opened read-only since `mark`, each once, in order.
    fn examined(&self, mark: usize) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for c in &self.server.commands()[mark..] {
            // `EXAMINE "<name>"`, perhaps with `(CONDSTORE)` after.
            if let Some(name) = c
                .strip_prefix("EXAMINE \"")
                .and_then(|r| r.split('"').next())
            {
                let name = name.to_owned();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }
}

#[tokio::test]
async fn mail_not_downloaded_yet_is_found_and_stored() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.user
        .append("INBOX", mail(Some("old@x"), "Old rota", ""))
        .unwrap();
    f.sync().await;
    // Arrived since the last pass: not here yet.
    f.user
        .append("Work", mail(Some("new@x"), "New rota", ""))
        .unwrap();
    f.user
        .append("INBOX", mail(Some("other@x"), "Other", ""))
        .unwrap();
    assert!(f.message("new@x").is_none());
    let found = f.search(&one(text("rota")), 50).await.unwrap();
    assert_eq!(found.incomplete, None);
    let mut ids = found.ids.clone();
    ids.sort();
    let mut want = vec![f.id("old@x"), f.id("new@x")];
    want.sort();
    assert_eq!(ids, want);
    assert_eq!(f.message("new@x").unwrap().subject(), "New rota");
    assert_eq!(f.labels("new@x"), [f.label_id("Work")]);
    assert!(
        f.message("other@x").is_none(),
        "only what was found is stored"
    );
    // A pass after keeps it as it is.
    f.sync().await;
    assert_eq!(f.message_count(), 3);
    assert_eq!(f.labels("new@x"), [f.label_id("Work")]);
}

#[tokio::test]
async fn the_page_keeps_the_newest_across_folders() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.sync().await;
    let at = |id: &str, day| mail(Some(id), "Rota", "").internal_date(on(day));
    f.user.append("INBOX", at("first@x", 1)).unwrap();
    f.user.append("INBOX", at("third@x", 20)).unwrap();
    f.user.append("Work", at("second@x", 10)).unwrap();
    let found = f.search(&one(text("rota")), 2).await.unwrap();
    assert_eq!(found.ids, [f.id("third@x"), f.id("second@x")]);
    assert!(f.message("first@x").is_none(), "past the page");
}

#[tokio::test]
async fn trash_and_junk_are_searched_only_when_asked() {
    let f = fixture().await;
    f.user.create_standard_folders().unwrap();
    f.sync().await;
    f.user
        .append("INBOX", mail(Some("i@x"), "Rota", ""))
        .unwrap();
    f.user
        .append("Trash", mail(Some("t@x"), "Rota", ""))
        .unwrap();
    f.user
        .append("Junk", mail(Some("j@x"), "Rota", ""))
        .unwrap();
    let mark = f.mark();
    let found = f.search(&one(text("rota")), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("i@x")]);
    let examined = f.examined(mark);
    assert_eq!(examined[0], "INBOX", "the inbox first");
    assert!(!examined.iter().any(|n| n == "Trash" || n == "Junk"));
    assert!(examined.iter().any(|n| n == "Archive"));
    // in:anywhere
    let mut anywhere = text("rota");
    anywhere.set_aside_too = true;
    let found = f.search(&one(anywhere), 50).await.unwrap();
    assert_eq!(found.ids.len(), 3);
}

#[tokio::test]
async fn named_folders_are_the_only_ones_searched() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.user.create_folder("Home").unwrap();
    f.sync().await;
    f.user
        .append("Work", mail(Some("w@x"), "Rota", ""))
        .unwrap();
    f.user
        .append("Home", mail(Some("h@x"), "Rota", ""))
        .unwrap();
    let mut work = text("rota");
    work.folders = vec![f.label_id("Work")];
    let mark = f.mark();
    let found = f.search(&one(work), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("w@x")]);
    assert_eq!(f.examined(mark), ["Work"]);
    // -in:Work
    let mut not_work = text("rota");
    not_work.not_in = vec![f.label_id("Work")];
    let found = f.search(&one(not_work), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("h@x")]);
}

#[tokio::test]
async fn a_busy_folder_is_left_out_and_said_so() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.sync().await;
    f.user
        .append("INBOX", mail(Some("i@x"), "Rota", ""))
        .unwrap();
    f.user
        .append("Work", mail(Some("w@x"), "Rota", ""))
        .unwrap();
    f.server.fail_next_matching(
        "EXAMINE",
        "\"Work\"",
        1,
        ImapFault::No("[INUSE] Mailbox is locked".into()),
    );
    let found = f.search(&one(text("rota")), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("i@x")]);
    let why = found.incomplete.expect("said so");
    assert!(why.contains("1 folder was not searched"), "{why}");
    assert!(why.contains("Mailbox is locked"), "{why}");
}

#[tokio::test]
async fn every_folder_busy_is_busy_not_refused() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.sync().await;
    f.user
        .append("Work", mail(Some("w@x"), "Rota", ""))
        .unwrap();
    // INBOX and Work, both locked.
    f.server.fail_next(
        "EXAMINE",
        2,
        ImapFault::No("[INUSE] Mailbox is locked".into()),
    );
    let result = f.search(&one(text("rota")), 50).await;
    match result {
        Err(SyncError::Busy { message, .. }) => {
            assert!(message.contains("Mailbox is locked"), "{message}");
        }
        other => panic!("busy, not {other:?}"),
    }
    // Busy for those folders, not the account: the next search asks.
    let found = f.search(&one(text("rota")), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("w@x")]);
}

#[tokio::test]
async fn a_page_slot_is_a_messages_not_a_copys() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.sync().await;
    let at = |id: &str, day| mail(Some(id), "Rota", "").internal_date(on(day));
    // One message filed in both folders, the newest.
    f.user.append("INBOX", at("both@x", 20)).unwrap();
    f.user.append("Work", at("both@x", 20)).unwrap();
    f.user.append("Work", at("ten@x", 10)).unwrap();
    f.user.append("INBOX", at("one@x", 1)).unwrap();
    let found = f.search(&one(text("rota")), 2).await.unwrap();
    assert_eq!(found.ids, [f.id("both@x"), f.id("ten@x")]);
    assert!(f.message("one@x").is_none(), "past the page");
}

#[tokio::test]
async fn a_busy_account_is_busy_not_a_sign_in() {
    let f = fixture().await;
    f.sync().await;
    f.user
        .append("INBOX", mail(Some("i@x"), "Rota", ""))
        .unwrap();
    f.server
        .fail_next("UID SEARCH", 1, ImapFault::No(BANDWIDTH.into()));
    let result = f.search(&one(text("rota")), 50).await;
    assert!(matches!(result, Err(SyncError::Busy { .. })), "{result:?}");
    let account = f.store.account(&f.me).unwrap().unwrap();
    assert_eq!(account.state, postrinch_model::AccountState::Active);
    // The next search is answered.
    let found = f.search(&one(text("rota")), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("i@x")]);
}

#[tokio::test]
async fn text_the_server_cannot_search_is_refused_with_why() {
    let f = fixture_with(ImapConfig::new().ascii_search(true), FolderRoles::default()).await;
    f.sync().await;
    f.user
        .append("INBOX", mail(Some("c@x"), "Caf\u{e9} rota", ""))
        .unwrap();
    let result = f.search(&one(text("caf\u{e9}")), 50).await;
    match result {
        Err(SyncError::SearchRefused { message, .. }) => {
            assert!(message.contains("not plain ASCII"), "{message}");
        }
        other => panic!("refused, not {other:?}"),
    }
    assert!(f.message("c@x").is_none());
    // ASCII text is searched as ever.
    let found = f.search(&one(text("rota")), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("c@x")]);
}

#[tokio::test]
async fn a_term_imap_cannot_ask_is_refused_before_asking() {
    let f = fixture().await;
    f.sync().await;
    let mut attached = text("rota");
    attached.inexact = Some("has:attachment".into());
    let result = f.search(&one(attached), 50).await;
    match result {
        Err(SyncError::SearchRefused { message, .. }) => {
            assert!(message.contains("has:attachment"), "{message}");
        }
        other => panic!("refused, not {other:?}"),
    }
    assert!(
        !f.server
            .commands()
            .iter()
            .any(|c| c.starts_with("UID SEARCH")),
        "nothing was asked"
    );
}

#[tokio::test]
async fn gmail_is_searched_in_all_mail_in_its_own_language() {
    let f = fixture_with(ImapConfig::new().gmail(true), FolderRoles::default()).await;
    f.user.create_noselect_folder("[Gmail]").unwrap();
    f.user
        .create_special_folder("[Gmail]/All Mail", SpecialUse::All)
        .unwrap();
    f.sync().await;
    for folder in ["INBOX", "[Gmail]/All Mail"] {
        f.user
            .append(
                folder,
                mail(Some("g@x"), "Rota", "Content-Type: text/plain\r\n"),
            )
            .unwrap();
    }
    let mut attached = text("rota");
    attached.gmail = "has:attachment rota".into();
    attached.inexact = Some("has:attachment".into());
    let mark = f.mark();
    let found = f.search(&one(attached), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("g@x")]);
    assert_eq!(f.examined(mark), ["[Gmail]/All Mail"]);
    assert!(
        f.server
            .commands()
            .iter()
            .any(|c| c == "UID SEARCH X-GM-RAW \"has:attachment rota\""),
        "{:?}",
        f.server.commands()
    );
}

#[tokio::test]
async fn nothing_to_ask_asks_nothing() {
    let f = fixture().await;
    f.sync().await;
    let before = f.server.commands().len();
    let found = f.search(&ImapSearch::default(), 50).await.unwrap();
    assert_eq!(found, ServerFound::default());
    assert_eq!(f.server.commands().len(), before);
}
