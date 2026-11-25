//! Opening what a server search found, and searching while the person
//! opens mail (docs/imap.md, "Searching the server"): a hit opens though
//! no pass has read its folder, or since its UIDVALIDITY changed; a
//! search has a connection of its own, so opening a message never waits
//! behind one; and a newer search takes an older one's place.

use std::pin::pin;
use std::time::Duration;

use postrinch_mailtest::ImapFault;
use postrinch_sync::SyncError;

use super::idle_support::sign_ins;
use super::keep_support::until_true;
use super::search::{one, text};
use super::sync_support::{fixture, mail};

#[tokio::test]
async fn a_hit_in_a_folder_no_pass_has_read_opens() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.user
        .append("Work", mail(Some("w@x"), "Rota", ""))
        .unwrap();
    // The first pass lists Work but cannot read it.
    f.server
        .fail_next_matching("SELECT", "\"Work\"", 1, ImapFault::No("not now".into()));
    f.sync().await;
    let work = f.label_id("Work");
    let state = |label: &str| f.store.imap_folder(&f.me, label).unwrap().expect("listed");
    assert_eq!(state(&work).uidvalidity, 0, "not read yet");
    assert!(f.message("w@x").is_none());

    let found = f.search(&one(text("rota")), 50).await.unwrap();
    assert_eq!(found.ids, [f.id("w@x")]);
    let opened = f.sync.download_now(&f.me, found.ids).await.unwrap();
    assert_eq!(opened, 1);
    assert!(f.message("w@x").unwrap().downloaded, "its body is here");
    assert_eq!(
        state(&work).uidvalidity,
        0,
        "the folder's state is the pass's"
    );

    // The pass that reads Work keeps it, body and all, once.
    f.sync().await;
    assert_ne!(state(&work).uidvalidity, 0);
    assert_eq!(f.message_count(), 1);
    assert!(f.message("w@x").unwrap().downloaded);
    assert_eq!(f.labels("w@x"), [work]);
}

#[tokio::test]
async fn a_hit_found_after_its_uidvalidity_changed_opens() {
    let f = fixture().await;
    f.user
        .append("INBOX", mail(Some("old@x"), "Old rota", ""))
        .unwrap();
    f.sync().await;
    // Numbered again since the last pass, with new mail.
    f.user.set_uid_validity("INBOX", 4242).unwrap();
    f.user
        .append("INBOX", mail(Some("new@x"), "New rota", ""))
        .unwrap();

    let found = f.search(&one(text("rota")), 50).await.unwrap();
    let mut ids = found.ids.clone();
    ids.sort();
    let mut want = vec![f.id("old@x"), f.id("new@x")];
    want.sort();
    assert_eq!(ids, want);
    // The old one's copy of the old UIDVALIDITY is no longer there; its
    // copy found by the search is.
    let opened = f.sync.download_now(&f.me, found.ids).await.unwrap();
    assert_eq!(opened, 2);
    assert!(f.message("old@x").unwrap().downloaded);
    assert!(f.message("new@x").unwrap().downloaded);

    // The pass reads the folder whole at its new UIDVALIDITY and keeps
    // both, with their bodies; neither is taken for gone.
    f.sync().await;
    f.sync().await;
    assert_eq!(f.message_count(), 2);
    assert!(f.message("old@x").unwrap().downloaded);
    assert!(f.message("new@x").unwrap().downloaded);
}

#[tokio::test]
async fn opening_a_message_does_not_wait_behind_a_slow_search() {
    let f = fixture().await;
    f.user
        .append("INBOX", mail(Some("open@x"), "Open me", ""))
        .unwrap();
    f.sync().await;
    let asked = f.server.command_count("UID SEARCH");
    let slow = f.server.hold_next("UID SEARCH", "");
    let (sync, me) = (f.sync.clone(), f.me.clone());
    let search = tokio::spawn(async move { sync.search_server(&me, &one(text("rota")), 50).await });
    until_true("the server is asked", || {
        f.server.command_count("UID SEARCH") > asked
    })
    .await;

    let opened = tokio::time::timeout(
        Duration::from_secs(5),
        f.sync.download_now(&f.me, vec![f.id("open@x")]),
    )
    .await
    .expect("opened while the search waits")
    .unwrap();
    assert_eq!(opened, 1);
    assert!(f.message("open@x").unwrap().downloaded);

    slow.let_go();
    assert!(search.await.unwrap().unwrap().ids.is_empty());
}

#[tokio::test]
async fn a_newer_search_takes_an_older_ones_place() {
    let f = fixture().await;
    f.user
        .append("INBOX", mail(Some("r@x"), "Rota", ""))
        .unwrap();
    f.sync().await;
    let asked = f.server.command_count("UID SEARCH");
    let signed_in = sign_ins(&f.server);
    let slow = f.server.hold_next("UID SEARCH", "");
    let (sync, me) = (f.sync.clone(), f.me.clone());
    let older = tokio::spawn(async move { sync.search_server(&me, &one(text("shift")), 50).await });
    until_true("the server is asked", || {
        f.server.command_count("UID SEARCH") > asked
    })
    .await;

    // The newer one begins, and waits for the older one's command.
    let newer = one(text("rota"));
    let mut newer = pin!(f.search(&newer, 50));
    assert!(
        tokio::time::timeout(Duration::ZERO, &mut newer)
            .await
            .is_err(),
        "it waits its turn"
    );
    slow.let_go();
    let older = older.await.unwrap();
    assert!(
        matches!(older, Err(SyncError::SearchSuperseded)),
        "{older:?}"
    );
    let found = newer.await.unwrap();
    assert_eq!(found.ids, [f.id("r@x")]);
    // Over the one search connection, kept when the older one stopped.
    assert_eq!(sign_ins(&f.server), signed_in + 1);
}
