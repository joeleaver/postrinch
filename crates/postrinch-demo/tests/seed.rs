//! Seeding a scratch store, then reading it back through the commands the
//! window uses.

use jiff::civil::date;
use jiff::tz::TimeZone;
use jiff::{ToSpan, Zoned};
use postrinch_commands::{Commands, ListOrder};
use postrinch_demo::{DemoError, SAM_HOME, SAM_WORK, seed};
use postrinch_model::{SavedView, Timestamp};
use postrinch_store::Store;

/// A Wednesday afternoon, in this machine's time zone (as the window reads
/// the calendar in).
fn now() -> Zoned {
    date(2026, 11, 25)
        .at(15, 30, 0, 0)
        .to_zoned(TimeZone::system())
        .unwrap()
}

fn seeded() -> (tempfile::TempDir, Commands) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("postrinch");
    let seeded = seed(&root, &now()).unwrap();
    assert_eq!(seeded.accounts, 2);
    assert!(
        (40..=60).contains(&seeded.threads),
        "{} threads",
        seeded.threads
    );
    assert!(seeded.messages > seeded.threads);
    assert_eq!((seeded.views, seeded.rules), (3, 3));
    (dir, Commands::new(Store::open(&root).unwrap()))
}

fn view(commands: &Commands, key: &str) -> SavedView {
    commands
        .views()
        .unwrap()
        .into_iter()
        .find(|e| e.view.key == key || e.view.name == key)
        .unwrap_or_else(|| panic!("no view {key}"))
        .view
}

fn subjects(commands: &Commands, view: &SavedView) -> Vec<String> {
    commands
        .list_view(view, ListOrder::NewestFirst, 0, 200)
        .unwrap()
        .threads
        .into_iter()
        .map(|t| t.summary.subject)
        .collect()
}

#[test]
fn the_inbox_lists_the_expected_threads() {
    let (_dir, commands) = seeded();
    let inbox = subjects(&commands, &view(&commands, "inbox"));
    assert_eq!(inbox[0], "Harbor Coffee rebrand: round 2 feedback");
    for expected in [
        "Dinner on Sunday?",
        "Climbing Tuesday?",
        "Your receipt from Paper Crane Shop (PC-20418)",
        "Menu boards: can we move the call?",
        "Moodboard v3 is up",
    ] {
        assert!(
            inbox.iter().any(|s| s == expected),
            "{expected} not in {inbox:?}"
        );
    }
    // Newsletters, shops and notifications are in their tabs, not Primary.
    assert!(!inbox.iter().any(|s| s.starts_with("Field Notes")));
    let updates = subjects(
        &commands,
        &view(&commands, &format!("updates:{}", SAM_HOME.address)),
    );
    assert!(updates.iter().any(|s| s.starts_with("Field Notes #112")));
    let promotions = subjects(
        &commands,
        &view(&commands, &format!("promotions:{}", SAM_WORK.address)),
    );
    assert!(promotions.contains(&"New this month: Juniper Serif".to_owned()));
}

#[test]
fn views_and_search_find_the_demo_mail() {
    let (_dir, commands) = seeded();
    let harbor = subjects(&commands, &view(&commands, "Harbor Coffee"));
    assert!(harbor.contains(&"Kickoff notes".to_owned()));
    assert!(harbor.iter().all(|s| !s.contains("Fernhill")));
    let found = commands.search("from:priya has:attachment", 50).unwrap();
    let found: Vec<_> = found
        .threads
        .iter()
        .map(|t| t.summary.subject.as_str())
        .collect();
    assert_eq!(found, ["Harbor Coffee rebrand: round 2 feedback"]);
}

#[test]
fn conversations_read_back_as_sent() {
    let (_dir, commands) = seeded();
    let inbox = view(&commands, "inbox");
    let page = commands
        .list_view(&inbox, ListOrder::NewestFirst, 0, 1)
        .unwrap();
    let row = &page.threads[0].summary;
    let thread = commands
        .read_thread(&row.account, &row.id)
        .unwrap()
        .unwrap();
    assert_eq!(thread.messages.len(), 6);
    let last = thread.messages.last().unwrap();
    assert!(last.flags.unread && row.starred);
    assert_eq!(
        last.headers.subject.as_deref(),
        Some("Re: Harbor Coffee rebrand: round 2 feedback")
    );
    // Each reply answers the one before it.
    for pair in thread.messages.windows(2) {
        assert_eq!(pair[1].headers.in_reply_to, pair[0].headers.message_id);
    }
    // Sam's own messages are in Sent.
    assert!(thread.messages[0].labels.contains(&"SENT".to_owned()));
    let notes = &thread.messages[3].attachments[0];
    assert_eq!(notes.filename.as_deref(), Some("round-2-notes.pdf"));
}

#[test]
fn the_invitation_matches_its_unanswered_event() {
    let (_dir, commands) = seeded();
    let work = SAM_WORK.email();
    let found = commands.search("subject:invitation", 10).unwrap();
    let row = &found.threads[0].summary;
    let thread = commands
        .read_thread(&row.account, &row.id)
        .unwrap()
        .unwrap();
    let message = &thread.messages[0];
    let invitation = commands.invitation(&work, &message.id).unwrap().unwrap();
    assert_eq!(invitation.uid, "hcpackaging2026@harborcoffee.example");
    assert_eq!(
        invitation.summary.as_deref(),
        Some("Harbor Coffee: packaging workshop")
    );

    let monday = now().date() - 2.days();
    let from = monday.at(0, 0, 0, 0).to_zoned(TimeZone::system()).unwrap();
    let to = from.checked_add(7.days()).unwrap();
    let millis = |z: &Zoned| Timestamp::from_millis(z.timestamp().as_millisecond());
    let events = commands
        .events_between(&[], millis(&from), millis(&to))
        .unwrap();
    let workshop = events
        .iter()
        .find(|h| h.event.summary.as_deref() == invitation.summary.as_deref())
        .unwrap();
    assert_eq!(workshop.account, work);
    assert!(workshop.event.guest && workshop.event.answer.is_none());
    assert_eq!(Some(workshop.event.when), invitation.when);
    let standups = events
        .iter()
        .filter(|h| h.event.summary.as_deref() == Some("Studio stand-up"))
        .count();
    assert_eq!(standups, 3, "Monday, Wednesday and Friday");
    assert!(
        events
            .iter()
            .any(|h| h.account.to_string() == SAM_HOME.address)
    );
}

#[test]
fn every_address_in_the_store_is_made_up() {
    let (_dir, commands) = seeded();
    let all = view(&commands, "all-mail");
    let page = commands
        .list_view(&all, ListOrder::NewestFirst, 0, 500)
        .unwrap();
    assert_eq!(page.total, page.threads.len());
    for row in &page.threads {
        for person in &row.summary.participants {
            let domain = person.address.domain().to_owned();
            assert!(
                ["example.com", "example.org", "example.net"].contains(&domain.as_str())
                    || domain.ends_with(".example"),
                "{} in {:?}",
                person.address,
                row.summary.subject
            );
        }
    }
}

#[test]
fn it_never_writes_over_a_store() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("something"), b"kept").unwrap();
    assert!(matches!(
        seed(dir.path(), &now()),
        Err(DemoError::NotEmpty(_))
    ));
    assert_eq!(
        std::fs::read(dir.path().join("something")).unwrap(),
        b"kept"
    );
}
