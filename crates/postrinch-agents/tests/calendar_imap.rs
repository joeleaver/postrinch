//! Calendars are Google accounts' only: every calendar call naming an
//! IMAP account is refused in a sentence saying so, before anything is
//! asked of the provider; a Google account beside it keeps its calendars.

use std::net::TcpListener;
use std::sync::Arc;

use postrinch_agents::client::Client;
use postrinch_agents::{Agents, Call, Reach};
use postrinch_commands::Commands;
use postrinch_model::{Account, EmailAddress, Provider, ProviderMessageId};
use postrinch_store::Store;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

/// Every calendar call that names an account, for `account`.
fn calendar_calls(account: &EmailAddress) -> Vec<Call> {
    let one = || account.clone();
    vec![
        Call::Events {
            accounts: vec![one()],
            from: "2026-11-24T00:00:00Z".into(),
            to: "2026-11-25T00:00:00Z".into(),
        },
        Call::FreeTime {
            accounts: vec![one()],
            from: "2026-11-24T08:00:00Z".into(),
            to: "2026-11-24T18:00:00Z".into(),
            minutes: 30,
            day_starts: None,
            day_ends: None,
            calendars: Vec::new(),
        },
        Call::SaveEvent {
            account: one(),
            calendar: None,
            event: None,
            summary: Some("Lunch".into()),
            starts: Some("2026-11-24T12:00:00Z".into()),
            ends: Some("2026-11-24T13:00:00Z".into()),
            first_day: None,
            last_day: None,
            location: None,
            description: None,
            guests: None,
            reason: None,
        },
        Call::DeleteEvent {
            account: one(),
            calendar: "primary".into(),
            event: "e1".into(),
            reason: None,
        },
        Call::AnswerEvent {
            account: one(),
            calendar: "primary".into(),
            event: "e1".into(),
            answer: "yes".into(),
            reason: None,
        },
        Call::AnswerInvitation {
            account: one(),
            message: ProviderMessageId("m1".into()),
            answer: "yes".into(),
            reason: None,
        },
        Call::CalendarShares {
            account: one(),
            calendar: "primary".into(),
        },
        Call::AddCalendar {
            account: one(),
            calendar: "team@example.org".into(),
        },
    ]
}

#[test]
fn calendar_calls_for_an_imap_account_are_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("store")).unwrap();
    let imap = address("ada@example.org");
    let google = address("me@example.org");
    store
        .put_account(&Account::new(imap.clone(), Provider::Imap))
        .unwrap();
    store
        .put_account(&Account::new(google.clone(), Provider::Google))
        .unwrap();
    let agents_path = dir.path().join("agents.json");
    let mut agents = Agents::load(&agents_path).unwrap();
    let (_, key) = agents.create("Test agent", Reach::Every).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    postrinch_agents::serve::start_on(
        listener,
        Commands::new(store.clone()),
        agents_path,
        postrinch_agents::serve::Hooks {
            on_change: Arc::new(|_| {}),
            remote: None,
        },
    )
    .unwrap();
    let client = Client::new(addr, Some(key));

    for call in calendar_calls(&imap) {
        let refused = client.call(&call).unwrap_err();
        assert_eq!(
            refused, "Calendars are Google accounts' only, and ada@example.org is an IMAP account.",
            "{call:?}"
        );
    }
    // A Google account's calendars are read as before.
    let events = client.call(&calendar_calls(&google)[0]).unwrap();
    assert!(events["events"].as_array().unwrap().is_empty(), "{events}");
    // And every account's calendars, IMAP accounts among them, are none
    // of the IMAP account's.
    let calendars = client.call(&Call::Calendars).unwrap();
    assert!(calendars.as_array().unwrap().is_empty(), "{calendars}");
}
