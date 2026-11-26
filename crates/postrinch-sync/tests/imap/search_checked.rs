//! What a server search finds is checked against the whole search as
//! search here reads it (docs/imap.md, "Searching the server"), and the
//! page is of messages that pass: newer finds the check leaves out do not
//! crowd out older ones that hold, and the walk stops at its bound.

use std::collections::HashSet;

use postrinch_mailtest::ImapFault;
use postrinch_model::ProviderMessageId;
use postrinch_sync::SyncError;
use postrinch_sync::imap::Holds;

use super::search::{on, one, text};
use super::sync_support::{Fixture, fixture, mail};

/// A check that leaves out the messages with these `Message-ID`s.
fn leaving_out(f: &Fixture, headers: &[&str]) -> Holds {
    let out: HashSet<ProviderMessageId> = headers.iter().map(|h| f.id(h)).collect();
    Holds::new(move |_, ids| Ok(ids.iter().filter(|id| !out.contains(id)).cloned().collect()))
}

#[tokio::test]
async fn newer_finds_left_out_do_not_crowd_out_older_ones_that_hold() {
    let f = fixture().await;
    f.sync().await;
    let rota = |id: &str| mail(Some(id), "Rota", "");
    f.user.append("INBOX", rota("old1@x")).unwrap();
    f.user.append("INBOX", rota("old2@x")).unwrap();
    // Newer than both, more than a page, and all left out by the check
    // (as `rota -from:billing` leaves out billing's).
    let billing: Vec<String> = (0..5).map(|n| format!("billing{n}@x")).collect();
    for id in &billing {
        f.user.append("INBOX", rota(id)).unwrap();
    }
    let out: Vec<&str> = billing.iter().map(String::as_str).collect();
    let found = f
        .search_checked(&one(text("rota")), &leaving_out(&f, &out), 2)
        .await
        .unwrap();
    assert_eq!(found.ids, [f.id("old2@x"), f.id("old1@x")]);
    assert_eq!(found.looked_through, None);
}

#[tokio::test]
async fn the_walk_across_folders_keeps_the_newest_that_hold() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.sync().await;
    let at = |id: &str, day| mail(Some(id), "Rota", "").internal_date(on(day));
    f.user.append("Work", at("work-old@x", 2)).unwrap();
    f.user.append("INBOX", at("inbox-old@x", 3)).unwrap();
    f.user.append("Work", at("work-mid@x", 5)).unwrap();
    for day in 10..14 {
        f.user
            .append("INBOX", at(&format!("no{day}@x"), day))
            .unwrap();
        f.user
            .append("Work", at(&format!("nw{day}@x"), day))
            .unwrap();
    }
    let out: Vec<String> = (10..14)
        .flat_map(|d| [format!("no{d}@x"), format!("nw{d}@x")])
        .collect();
    let out: Vec<&str> = out.iter().map(String::as_str).collect();
    let found = f
        .search_checked(&one(text("rota")), &leaving_out(&f, &out), 2)
        .await
        .unwrap();
    assert_eq!(found.ids, [f.id("work-mid@x"), f.id("inbox-old@x")]);
    assert!(f.message("work-old@x").is_none(), "past the page");
}

#[tokio::test]
async fn the_walk_stops_at_its_bound_and_says_so() {
    let f = fixture().await;
    f.sync().await;
    // A page of 1 looks through at most 10 found.
    f.user
        .append("INBOX", mail(Some("oldest@x"), "Rota", ""))
        .unwrap();
    let newer: Vec<String> = (0..12).map(|n| format!("n{n}@x")).collect();
    for id in &newer {
        f.user.append("INBOX", mail(Some(id), "Rota", "")).unwrap();
    }
    let out: Vec<&str> = newer.iter().map(String::as_str).collect();
    let found = f
        .search_checked(&one(text("rota")), &leaving_out(&f, &out), 1)
        .await
        .unwrap();
    assert_eq!(found.ids, Vec::<ProviderMessageId>::new());
    assert_eq!(found.looked_through, Some(10));
    assert!(f.message("n1@x").is_none(), "past the bound, not fetched");
    assert!(f.message("n2@x").is_some(), "within the bound");
    // Fewer found than the bound: nothing was cut short.
    let found = f
        .search_checked(&one(text("rota")), &leaving_out(&f, &out), 20)
        .await
        .unwrap();
    assert_eq!(found.ids, [f.id("oldest@x")]);
    assert_eq!(found.looked_through, None);
}

#[tokio::test]
async fn the_bound_is_on_what_is_walked_so_late_folders_compete() {
    // A page of 2 looks through at most 20 found. With 25 folders each
    // finding one, taking every folder's next to be in hand must not eat
    // that bound: the newest two, in the last folders, are the page.
    let f = fixture().await;
    let names: Vec<String> = (0..25).map(|n| format!("F{n:02}")).collect();
    for name in &names {
        f.user.create_folder(name).unwrap();
    }
    f.sync().await;
    for (day, name) in (1..).zip(&names) {
        let m = mail(Some(&format!("{name}@x")), "Rota", "").internal_date(on(day));
        f.user.append(name, m).unwrap();
    }
    let all = Holds::new(|_, ids| Ok(ids.iter().cloned().collect()));
    let found = f.search_checked(&one(text("rota")), &all, 2).await.unwrap();
    assert_eq!(found.ids, [f.id("F24@x"), f.id("F23@x")]);
    assert_eq!(found.looked_through, None);
    // With none holding, the bound stops the walk at the 20 newest, and
    // says so: the five oldest were not walked.
    let none = Holds::new(|_, _| Ok(HashSet::new()));
    let found = f
        .search_checked(&one(text("rota")), &none, 2)
        .await
        .unwrap();
    assert_eq!(found.looked_through, Some(20));
    assert!(f.message("F05@x").is_some(), "within the bound");
    assert!(f.message("F04@x").is_none(), "past the bound, not fetched");
}

#[tokio::test]
async fn a_copy_in_a_busy_folder_gives_way_to_another() {
    let f = fixture().await;
    f.user.create_folder("Work").unwrap();
    f.sync().await;
    // One message, its copy in Work the newer: walked first, but Work is
    // busy when its headers are fetched.
    let at = |day| mail(Some("both@x"), "Rota", "").internal_date(on(day));
    f.user.append("INBOX", at(3)).unwrap();
    f.user.append("Work", at(5)).unwrap();
    f.server.fail_next_matching(
        "UID FETCH",
        "BODY.PEEK[HEADER]",
        1,
        ImapFault::No("[INUSE] Mailbox is locked".into()),
    );
    let all = Holds::new(|_, ids| Ok(ids.iter().cloned().collect()));
    let found = f.search_checked(&one(text("rota")), &all, 1).await.unwrap();
    assert_eq!(
        found.ids,
        [f.id("both@x")],
        "the inbox's copy stands for it"
    );
}

#[tokio::test]
async fn a_check_that_fails_fails_the_search() {
    let f = fixture().await;
    f.sync().await;
    f.user
        .append("INBOX", mail(Some("a@x"), "Rota", ""))
        .unwrap();
    let broken = Holds::new(|_, _| Err("the query does not read".to_owned()));
    let found = f.search_checked(&one(text("rota")), &broken, 5).await;
    match found {
        Err(SyncError::SearchCheck(why)) => assert_eq!(why, "the query does not read"),
        other => panic!("a failed check, not {other:?}"),
    }
}
