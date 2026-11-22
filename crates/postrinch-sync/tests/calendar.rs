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
