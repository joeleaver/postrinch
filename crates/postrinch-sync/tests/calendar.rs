//! Invitations found and answered in the account's calendar.

mod common;

use postrinch_model::{Answer, Invitation, InvitationKind};
use postrinch_sync::google::Answered;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

fn invitation(uid: &str) -> Invitation {
    Invitation {
        kind: InvitationKind::Request,
        uid: uid.into(),
        recurrence: None,
        summary: Some("Plans".into()),
        location: None,
        when: None,
        repeats: false,
        organizer: None,
        attendees: Vec::new(),
    }
}

fn event(answer: &str) -> serde_json::Value {
    json!({
        "id": "e1",
        "status": "confirmed",
        "summary": "Plans",
        "start": {"dateTime": "2026-11-24T10:00:00-05:00"},
        "end": {"dateTime": "2026-11-24T11:00:00-05:00"},
        "attendees": [
            {"email": "ada@example.org", "responseStatus": "accepted", "organizer": true},
            {"email": "me@example.org", "responseStatus": answer, "self": true},
        ],
    })
}

const EVENTS: &str = "/gmail/calendar/calendars/primary/events";

#[tokio::test]
async fn finds_and_answers_an_invitation() {
    let f = common::fixture().await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .and(query_param("iCalUID", "abc@google.com"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [event("needsAction")]})),
        )
        .mount(&f.server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("{EVENTS}/e1")))
        .and(query_param("sendUpdates", "all"))
        // The whole guest list goes back, the organizer's as it was.
        .and(body_partial_json(json!({"attendees": [
            {"email": "ada@example.org", "responseStatus": "accepted", "organizer": true},
            {"email": "me@example.org", "responseStatus": "tentative", "self": true},
        ]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(event("tentative")))
        .expect(1)
        .mount(&f.server)
        .await;

    let inv = invitation("abc@google.com");
    let found = f.sync.invitation_event(&f.me, &inv).await.unwrap().unwrap();
    assert_eq!((found.id.as_str(), found.answer), ("e1", None));

    let answered = f
        .sync
        .answer_invitation(&f.me, &inv, Answer::Maybe)
        .await
        .unwrap();
    let Answered::Done(event) = answered else {
        panic!("{answered:?}")
    };
    assert_eq!(event.answer, Some(Answer::Maybe));
}

#[tokio::test]
async fn an_event_not_in_the_calendar_is_said_so() {
    let f = common::fixture().await;
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": []})))
        .mount(&f.server)
        .await;
    let inv = invitation("elsewhere@example.org");
    assert_eq!(f.sync.invitation_event(&f.me, &inv).await.unwrap(), None);
    assert_eq!(
        f.sync
            .answer_invitation(&f.me, &inv, Answer::Yes)
            .await
            .unwrap(),
        Answered::NotInCalendar
    );
}

#[tokio::test]
async fn an_organizer_is_not_a_guest() {
    let f = common::fixture().await;
    let mine = json!({
        "id": "e2",
        "start": {"date": "2026-12-24"},
        "end": {"date": "2026-12-25"},
        "attendees": [{"email": "ada@example.org", "responseStatus": "needsAction"}],
    });
    Mock::given(method("GET"))
        .and(path(EVENTS))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [mine]})))
        .mount(&f.server)
        .await;
    let inv = invitation("mine@google.com");
    assert_eq!(
        f.sync
            .answer_invitation(&f.me, &inv, Answer::No)
            .await
            .unwrap(),
        Answered::NotAGuest
    );
}

const LIST: &str = "/gmail/calendar/users/me/calendarList";
/// The account's own calendar's events, by its id.
const MINE: &str = "/gmail/calendar/calendars/me%40example.org/events";

fn timed(id: &str, summary: &str) -> serde_json::Value {
    json!({
        "id": id,
        "status": "confirmed",
        "summary": summary,
        "start": {"dateTime": "2026-11-24T10:00:00Z"},
        "end": {"dateTime": "2026-11-24T11:00:00Z"},
    })
}

#[tokio::test]
async fn keeps_calendars_and_their_events_in_step() {
    use wiremock::matchers::query_param_is_missing;
    let f = common::fixture().await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [
            {"id": "me@example.org", "summary": "me@example.org", "primary": true,
             "accessRole": "owner", "selected": true, "timeZone": "Europe/London"},
        ]})))
        .mount(&f.server)
        .await;
    // The first sync: two pages, then where to start next time.
    Mock::given(method("GET"))
        .and(path(MINE))
        .and(query_param_is_missing("syncToken"))
        .and(query_param_is_missing("pageToken"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [timed("a", "A")], "nextPageToken": "p2"})))
        .mount(&f.server)
        .await;
    Mock::given(method("GET"))
        .and(path(MINE))
        .and(query_param("pageToken", "p2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [timed("b", "B"), timed("c", "C")], "nextSyncToken": "t1"})))
        .mount(&f.server)
        .await;
    let report = f.sync.sync_calendars(&f.me).await.unwrap();
    assert_eq!((report.calendars, report.kept), (1, 3));
    assert_eq!(
        f.store
            .calendar_sync_token(&f.me, "me@example.org")
            .unwrap()
            .as_deref(),
        Some("t1")
    );

    // Then only what changed: B renamed, C deleted.
    Mock::given(method("GET"))
        .and(path(MINE))
        .and(query_param("syncToken", "t1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [timed("b", "B, moved"), {"id": "c", "status": "cancelled"}],
            "nextSyncToken": "t2"})))
        .mount(&f.server)
        .await;
    let report = f.sync.sync_calendars(&f.me).await.unwrap();
    assert_eq!((report.kept, report.removed), (1, 1));
    let mut kept: Vec<_> = f
        .store
        .events(&f.me)
        .unwrap()
        .into_iter()
        .map(|(_, r)| r.event.summary.unwrap())
        .collect();
    kept.sort();
    assert_eq!(kept, ["A", "B, moved"]);

    // A calendar that forgot where it was starts again.
    Mock::given(method("GET"))
        .and(path(MINE))
        .and(query_param("syncToken", "t2"))
        .respond_with(ResponseTemplate::new(410).set_body_json(json!({
            "error": {"code": 410, "message": "Sync token is no longer valid", "errors": []}})))
        .mount(&f.server)
        .await;
    // Listed again from the start: A as it was, B back as it was first, C
    // again; only what changed is written.
    let report = f.sync.sync_calendars(&f.me).await.unwrap();
    assert_eq!((report.kept, report.removed), (2, 0));
    assert_eq!(f.store.events(&f.me).unwrap().len(), 3);
    assert_eq!(
        f.store
            .calendar_sync_token(&f.me, "me@example.org")
            .unwrap()
            .as_deref(),
        Some("t1")
    );
    // A calendar that refused its token rests before the next full sync.
    let report = f.sync.sync_calendars(&f.me).await.unwrap();
    assert_eq!((report.kept, report.removed), (0, 0));
}

#[tokio::test]
async fn a_calendar_no_longer_listed_goes() {
    let f = common::fixture().await;
    let info = postrinch_model::CalendarInfo {
        id: "old".into(),
        name: "Old".into(),
        color: None,
        primary: false,
        writable: false,
        shown: true,
        time_zone: None,
        reminders: Vec::new(),
    };
    f.store.put_calendar(&f.me, &info).unwrap();
    Mock::given(method("GET"))
        .and(path(LIST))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": []})))
        .mount(&f.server)
        .await;
    f.sync.sync_calendars(&f.me).await.unwrap();
    assert!(f.store.calendars(&f.me).unwrap().is_empty());
}

#[tokio::test]
async fn makes_changes_and_deletes_events_keeping_guests_answers() {
    use postrinch_model::{Day, EmailAddress, EventDraft, EventWhen, Mailbox};
    let f = common::fixture().await;
    let guest = |a: &str| Mailbox::new(None, EmailAddress::parse(a).unwrap());
    let d = Day {
        year: 2026,
        month: 12,
        day: 24,
    };
    let mut draft = EventDraft {
        calendar: "me@example.org".into(),
        summary: "Walkthrough".into(),
        location: String::new(),
        description: String::new(),
        when: EventWhen::Days { first: d, last: d },
        time_zone: None,
        guests: vec![guest("bob@example.org")],
    };
    Mock::given(method("POST"))
        .and(path(MINE))
        .and(query_param("sendUpdates", "all"))
        .and(body_partial_json(json!({
            "summary": "Walkthrough",
            "start": {"date": "2026-12-24"}, "end": {"date": "2026-12-25"},
            "attendees": [{"email": "bob@example.org"}],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "n1"})))
        .expect(1)
        .mount(&f.server)
        .await;
    assert_eq!(f.sync.create_event(&f.me, &draft).await.unwrap(), "n1");

    // Bob stays with his answer, Carol is new, Dan goes.
    Mock::given(method("GET"))
        .and(path(format!("{MINE}/n1")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "n1",
            "attendees": [
                {"email": "me@example.org", "self": true, "organizer": true, "responseStatus": "accepted"},
                {"email": "bob@example.org", "responseStatus": "accepted"},
                {"email": "dan@example.org", "responseStatus": "needsAction"},
            ],
        })))
        .mount(&f.server)
        .await;
    draft.guests = vec![guest("bob@example.org"), guest("carol@example.org")];
    Mock::given(method("PATCH"))
        .and(path(format!("{MINE}/n1")))
        .and(query_param("sendUpdates", "all"))
        .and(body_partial_json(json!({"attendees": [
            {"email": "me@example.org", "self": true, "organizer": true, "responseStatus": "accepted"},
            {"email": "bob@example.org", "responseStatus": "accepted"},
            {"email": "carol@example.org"},
        ]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "n1"})))
        .expect(1)
        .mount(&f.server)
        .await;
    f.sync.change_event(&f.me, "n1", &draft).await.unwrap();

    Mock::given(method("DELETE"))
        .and(path(format!("{MINE}/n1")))
        .and(query_param("sendUpdates", "all"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&f.server)
        .await;
    f.sync
        .delete_event(&f.me, "me@example.org", "n1", true)
        .await
        .unwrap();
}
