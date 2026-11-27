//! Two providers: remote key fetches (§4.6) and direct messages (§7.4).

use crate::common::{Ws, user};
use ofscp_core::DmId;
use ofscp_core::ws::Event;
use postrinch_ofscp_fake::{Authentication, FakeProvider};
use reqwest::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn a_foreign_actor_is_verified_with_its_home_keys() {
    let a = FakeProvider::start().await.unwrap();
    let b = FakeProvider::start().await.unwrap();
    let alice = user(&a, "alice");
    let bob = user(&b, "bob");
    let group = a.create_group(&alice.actor, "Team");
    a.add_member(&group, &bob.actor, "member");
    let r = bob
        .send(
            &a,
            Method::GET,
            &format!("/api/groups/{}", group.as_str()),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["membership"]["role"], "member");
    assert_eq!(
        a.requests().pop().unwrap().authentication,
        Authentication::Verified {
            actor: bob.actor.clone(),
            key_id: bob.key_id.clone(),
        }
    );
    // b served its keys to a.
    assert!(
        b.requests()
            .iter()
            .any(|r| r.path == "/.well-known/ofscp/users/bob/keys")
    );
    // Local-account endpoints refuse a foreign actor.
    let r = bob.send(&a, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_foreign_key_that_is_gone_does_not_verify() {
    let a = FakeProvider::start().await.unwrap();
    let b = FakeProvider::start().await.unwrap();
    let bob = user(&b, "bob");
    b.faults().revoke_key(&bob.key_id);
    let r = bob.send(&a, Method::GET, "/.well-known/ofscp", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_failed_key_fetch_is_503_not_401() {
    let a = FakeProvider::start().await.unwrap();
    let b = FakeProvider::start().await.unwrap();
    let bob = user(&b, "bob");
    // Bob's home goes away: a cannot fetch his keys.
    drop(b);
    let r = bob.send(&a, Method::GET, "/.well-known/ofscp", None).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{}", r.json);
}

fn dm_body(client_id: &str, to: &ofscp_core::Actor, text: &str) -> serde_json::Value {
    json!({
        "clientMessageId": client_id,
        "to": to.to_string(),
        "content": {"text": text, "mime": "text/plain"},
    })
}

#[tokio::test]
async fn a_dm_is_stored_at_the_recipients_home_and_delivered_live() {
    let a = FakeProvider::start().await.unwrap();
    let b = FakeProvider::start().await.unwrap();
    let alice = user(&a, "alice");
    let bob = user(&b, "bob");
    let dm = DmId::derive(&alice.actor, &bob.actor);
    let mut alice_ws = Ws::signed_in(&a, &alice).await;

    let path = format!("/api/federation/dms/{}/messages", dm.as_str());
    let body = dm_body("d-1", &alice.actor, "hi alice");
    let r = bob.send(&a, Method::POST, &path, Some(&body)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.json);
    let id = r.json["id"].as_str().unwrap().to_owned();
    // Idempotent.
    let again = bob.send(&a, Method::POST, &path, Some(&body)).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json["id"], id.as_str());

    // Implicit delivery: alice is not subscribed to the dmId.
    match alice_ws.event().await.body {
        Event::DmMessage(m) => assert_eq!(m.message.content.unwrap().text, "hi alice"),
        other => panic!("expected dm.message, got {other:?}"),
    }

    // Both participants read it at alice's home; a third party cannot.
    let history = format!("/api/dms/{}/messages", dm.as_str());
    let r = alice.send(&a, Method::GET, &history, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["items"][0]["id"], id.as_str());
    let r = bob.send(&a, Method::GET, &history, None).await;
    assert_eq!(r.status, StatusCode::OK);
    let carol = user(&b, "carol");
    let r = carol.send(&a, Method::GET, &history, None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // A wrong `to` is refused.
    let r = bob
        .send(
            &a,
            Method::POST,
            &path,
            Some(&dm_body("d-2", &bob.actor, "x")),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Alice's list shows the conversation.
    let r = alice.send(&a, Method::GET, "/api/me/dms", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["items"][0]["id"], dm.as_str());
}

#[tokio::test]
async fn the_sender_keeps_a_conversation_pointer_at_home() {
    let a = FakeProvider::start().await.unwrap();
    let b = FakeProvider::start().await.unwrap();
    let alice = user(&a, "alice");
    let bob = user(&b, "bob");
    let dm = DmId::derive(&alice.actor, &bob.actor);
    let path = format!("/api/me/dms/{}", dm.as_str());
    let r = bob
        .send(
            &b,
            Method::PUT,
            &path,
            Some(&json!({"with": alice.actor.to_string()})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["id"], dm.as_str());
    let wrong = json!({"with": bob.actor.to_string()});
    let r = bob.send(&b, Method::PUT, &path, Some(&wrong)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = bob.send(&b, Method::GET, "/api/me/dms", None).await;
    assert_eq!(r.json["items"].as_array().unwrap().len(), 1);
}
