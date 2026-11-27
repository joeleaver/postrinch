//! Groups, channel messages, reactions and the home feeds over REST.

use crate::common::{Ws, user};
use ofscp_core::wire::JoinPolicy;
use ofscp_core::ws::Event;
use postrinch_ofscp_fake::FakeProvider;
use reqwest::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn joining_and_leaving_follow_the_join_policy() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let bob = user(&p, "bob");
    let open = p.create_group(&alice.actor, "Open");
    p.configure_group(&open, |g| g.tier = ofscp_core::wire::Tier::Public);
    let mut bob_ws = Ws::signed_in(&p, &bob).await;

    let join = format!("/api/groups/{}/join", open.as_str());
    let r = bob.send(&p, Method::POST, &join, Some(&json!({}))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.json);
    assert_eq!(r.json["role"], "member");
    assert!(matches!(bob_ws.event().await.body, Event::GroupsUpdated(_)));
    assert_eq!(
        bob.send(&p, Method::POST, &join, None).await.status,
        StatusCode::OK
    );
    let r = bob.send(&p, Method::GET, "/api/me/groups", None).await;
    assert_eq!(r.json["groups"].as_array().unwrap().len(), 1, "{}", r.json);

    let leave = format!("/api/groups/{}/leave", open.as_str());
    assert_eq!(
        bob.send(&p, Method::POST, &leave, None).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        alice.send(&p, Method::POST, &leave, None).await.status,
        StatusCode::CONFLICT
    );

    let asks = p.create_group(&alice.actor, "Asks");
    p.configure_group(&asks, |g| {
        g.tier = ofscp_core::wire::Tier::Public;
        g.join_policy = JoinPolicy::Request;
    });
    let r = bob
        .send(
            &p,
            Method::POST,
            &format!("/api/groups/{}/join", asks.as_str()),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert_eq!(r.json["state"], "pending");
}

#[tokio::test]
async fn messages_are_paged_edited_deleted_and_reacted_to() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let group = p.create_group(&alice.actor, "Team");
    let channel = p.create_channel(&group, "general");
    for n in 0..3 {
        p.post_as(&alice.actor, &group, &channel, &format!("m{n}"));
    }
    let base = format!(
        "/api/groups/{}/channels/{}/messages",
        group.as_str(),
        channel.as_str()
    );
    let r = alice
        .send(&p, Method::GET, &format!("{base}?limit=2"), None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["items"][0]["content"]["text"], "m2");
    let next = r.json["page"]["nextCursor"].as_str().unwrap().to_owned();
    let r = alice
        .send(
            &p,
            Method::GET,
            &format!("{base}?limit=2&cursor={next}"),
            None,
        )
        .await;
    assert_eq!(r.json["items"].as_array().unwrap().len(), 1);
    assert_eq!(r.json["items"][0]["content"]["text"], "m0");

    let id = p.messages(&group, &channel)[0].id.clone();
    let one = format!("{base}/{}", id.as_str());
    let edit = json!({"content": {"text": "m0 edited", "mime": "text/plain"}});
    let r = alice.send(&p, Method::PATCH, &one, Some(&edit)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["content"]["text"], "m0 edited");

    let reaction = format!("{one}/reactions/heart");
    let r = alice
        .send(&p, Method::PUT, &reaction, Some(&json!({"unicode": "❤"})))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.json);
    assert_eq!(
        alice.send(&p, Method::PUT, &reaction, None).await.status,
        StatusCode::OK
    );
    let r = alice
        .send(&p, Method::GET, &format!("{one}/reactions"), None)
        .await;
    assert_eq!(r.json["items"][0]["key"], "heart");
    assert_eq!(
        alice.send(&p, Method::DELETE, &reaction, None).await.status,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        alice.send(&p, Method::DELETE, &one, None).await.status,
        StatusCode::NO_CONTENT
    );
    let stored = &p.messages(&group, &channel)[0];
    assert!(stored.deleted_at.is_some() && stored.content.is_none());
}

#[tokio::test]
async fn a_mention_notifies_the_mentioned_user() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let bob = user(&p, "bob");
    let group = p.create_group(&alice.actor, "Team");
    let channel = p.create_channel(&group, "general");
    p.add_member(&group, &bob.actor, "member");
    p.post_as(
        &alice.actor,
        &group,
        &channel,
        &format!("hi @{}", bob.actor),
    );
    let r = bob
        .send(&p, Method::GET, "/api/me/notifications", None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["items"][0]["type"], "mention", "{}", r.json);
    let r = bob
        .send(
            &p,
            Method::POST,
            "/api/me/notifications/read",
            Some(&json!({})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
}

#[tokio::test]
async fn follows_and_read_markers() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let group = p.create_group(&alice.actor, "Team");
    let channel = p.create_channel(&group, "general");
    let m = p.post_as(&alice.actor, &group, &channel, "read me");
    let body = json!({"channel": channel.as_str(), "groupId": group.as_str()});
    let r = alice
        .send(&p, Method::POST, "/api/me/follows", Some(&body))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.json);
    assert_eq!(
        alice
            .send(&p, Method::POST, "/api/me/follows", Some(&body))
            .await
            .status,
        StatusCode::OK
    );

    let markers =
        json!({"markers": [{"scopeId": channel.as_str(), "lastReadMessageId": m.id.as_str()}]});
    let r = alice
        .send(&p, Method::PATCH, "/api/me/read-markers", Some(&markers))
        .await;
    assert!(r.status.is_success(), "{} {}", r.status, r.json);
    let r = alice
        .send(&p, Method::GET, "/api/me/read-markers", None)
        .await;
    let found = r.json["scopes"].as_array().unwrap();
    assert!(
        found
            .iter()
            .any(|k| k["scopeId"] == channel.as_str() && k["lastReadMessageId"] == m.id.as_str()),
        "{}",
        r.json
    );
}

#[tokio::test]
async fn notification_settings_are_served_as_seeded() {
    use ofscp_core::wire::{NotificationMode, NotificationScope, NotificationSetting};

    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let r = alice
        .send(&p, Method::GET, "/api/me/notification-settings", None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json, json!({"prefs": []}));

    let group = p.create_group(&alice.actor, "Group");
    let setting = NotificationSetting {
        scope_type: NotificationScope::Group,
        scope_id: group.clone(),
        mode: NotificationMode::Mentions,
    };
    p.set_notification_settings(&alice.actor, vec![setting]);
    let r = alice
        .send(&p, Method::GET, "/api/me/notification-settings", None)
        .await;
    assert_eq!(
        r.json,
        json!({"prefs": [{"scopeType": "group", "scopeId": group.as_str(), "mode": "mentions"}]})
    );
}
