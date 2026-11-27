//! Invites and join requests (§5.6, §5.7): made, listed, revoked and
//! redeemed, and requests listed and answered, with the statuses and
//! rules the spec gives.

use crate::common::{User, user};
use ofscp_core::wire::JoinPolicy;
use ofscp_core::{Actor, LocalId};
use postrinch_ofscp_fake::FakeProvider;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

/// A group Alice owns, invite only, with a channel, and Bob, a member.
fn team(p: &FakeProvider) -> (User, User, LocalId, LocalId) {
    let alice = user(p, "alice");
    let bob = user(p, "bob");
    let group = p.create_group(&alice.actor, "Team");
    p.configure_group(&group, |g| g.join_policy = JoinPolicy::Invite);
    let channel = p.create_channel(&group, "general");
    p.add_member(&group, &bob.actor, "member");
    (alice, bob, group, channel)
}

async fn invite(p: &FakeProvider, by: &User, group: &LocalId, body: Value) -> (StatusCode, Value) {
    let path = format!("/api/groups/{}/invites", group.as_str());
    let r = by.send(p, Method::POST, &path, Some(&body)).await;
    (r.status, r.json)
}

async fn redeem(p: &FakeProvider, by: &User, token: &str) -> (StatusCode, Value) {
    let r = by
        .send(p, Method::POST, &format!("/api/invites/{token}/redeem"), None)
        .await;
    (r.status, r.json)
}

fn role_of(p: &FakeProvider, group: &LocalId, actor: &Actor) -> Option<String> {
    p.roles(group).get(actor).cloned()
}

#[tokio::test]
async fn an_invite_is_made_listed_without_its_token_and_revoked() {
    let p = FakeProvider::start().await.unwrap();
    let (alice, bob, group, channel) = team(&p);
    let (status, made) = invite(
        &p,
        &alice,
        &group,
        json!({"channelId": channel.as_str(), "maxUses": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let token = made["token"].as_str().unwrap();
    assert!(token.len() >= 22, "at least 128 bits: {token}");
    assert_eq!(made["uses"], 0);

    let list = format!("/api/groups/{}/invites", group.as_str());
    let r = alice.send(&p, Method::GET, &list, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let [listed] = &r.json["items"].as_array().unwrap()[..] else {
        panic!("one invite: {}", r.json);
    };
    assert_eq!(listed["id"], made["id"]);
    assert!(listed.get("token").is_none(), "the token is never listed");
    // A member who may not manage the group may not see them.
    let r = bob.send(&p, Method::GET, &list, None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let one = format!("{list}/{}", made["id"].as_str().unwrap());
    let r = bob.send(&p, Method::DELETE, &one, None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = alice.send(&p, Method::DELETE, &one, None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(p.invites(&group).is_empty());
    let r = alice.send(&p, Method::DELETE, &one, None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // A revoked invite's token finds nothing.
    let carol = user(&p, "carol");
    assert_eq!(redeem(&p, &carol, token).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_invite_keeps_to_the_groups_roles_channels_and_its_makers_rights() {
    let p = FakeProvider::start().await.unwrap();
    let (alice, bob, group, _) = team(&p);
    let dan = user(&p, "dan");
    p.add_member(&group, &dan.actor, "admin");
    let other = p.create_group(&alice.actor, "Other");
    let elsewhere = p.create_channel(&other, "elsewhere");

    let cases = [
        (&alice, json!({"role": "owner"}), StatusCode::BAD_REQUEST),
        (&alice, json!({"role": "wizard"}), StatusCode::BAD_REQUEST),
        (
            &alice,
            json!({"channelId": elsewhere.as_str()}),
            StatusCode::BAD_REQUEST,
        ),
        // Bob may not manage the group at all.
        (&bob, json!({}), StatusCode::FORBIDDEN),
        // An admin may hand out admin, as the owner may.
        (&dan, json!({"role": "admin"}), StatusCode::CREATED),
        (&alice, json!({"role": "moderator"}), StatusCode::CREATED),
    ];
    for (by, body, expected) in cases {
        let (status, json) = invite(&p, by, &group, body.clone()).await;
        assert_eq!(status, expected, "{body} → {json}");
    }

    // The subset rule (§5.7): a manager without `moderate` cannot grant it.
    p.configure_group(&group, |g| {
        g.permissions.manage = Some(vec!["admin".into(), "member".into()]);
    });
    let (status, json) = invite(&p, &bob, &group, json!({"role": "admin"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let (status, json) = invite(&p, &bob, &group, json!({"role": "member"})).await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
}

#[tokio::test]
async fn redeeming_admits_once_per_use_and_refuses_what_is_spent_or_gone() {
    let p = FakeProvider::start().await.unwrap();
    let (alice, _, group, channel) = team(&p);
    let carol = user(&p, "carol");
    let erin = user(&p, "erin");
    let (_, made) = invite(
        &p,
        &alice,
        &group,
        json!({"channelId": channel.as_str(), "maxUses": 1, "role": "moderator"}),
    )
    .await;
    let token = made["token"].as_str().unwrap();

    let (status, joined) = redeem(&p, &carol, token).await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["groupId"], group.as_str());
    assert_eq!(joined["channelId"], channel.as_str());
    assert_eq!(joined["role"], "moderator");
    assert_eq!(role_of(&p, &group, &carol.actor).as_deref(), Some("moderator"));
    assert_eq!(p.invites(&group)[0].uses, 1);

    // Spent: 409 for anyone, a member too, before membership is looked at.
    assert_eq!(redeem(&p, &erin, token).await.0, StatusCode::CONFLICT);
    assert_eq!(redeem(&p, &carol, token).await.0, StatusCode::CONFLICT);
    assert_eq!(role_of(&p, &group, &erin.actor), None);

    // An unknown token, and an expired invite, are not found.
    assert_eq!(redeem(&p, &erin, "nope").await.0, StatusCode::NOT_FOUND);
    let past = p.now().checked_sub(jiff::SignedDuration::from_secs(60)).unwrap();
    let (_, expired) = invite(&p, &alice, &group, json!({"expiresAt": past.to_string()})).await;
    let expired = expired["token"].as_str().unwrap();
    assert_eq!(redeem(&p, &erin, expired).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_member_redeeming_keeps_their_role_and_spends_no_use() {
    let p = FakeProvider::start().await.unwrap();
    let (alice, bob, group, channel) = team(&p);
    let (_, made) = invite(
        &p,
        &alice,
        &group,
        json!({"channelId": channel.as_str(), "role": "admin"}),
    )
    .await;
    let (status, joined) = redeem(&p, &bob, made["token"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["role"], "member", "the role Bob holds, not the invite's");
    assert_eq!(p.invites(&group)[0].uses, 0);
}

#[tokio::test]
async fn someone_on_another_provider_redeems_an_invite() {
    let p = FakeProvider::start().await.unwrap();
    let away = FakeProvider::start().await.unwrap();
    let (alice, _, group, _) = team(&p);
    let zoe = user(&away, "zoe");
    let (_, made) = invite(&p, &alice, &group, json!({})).await;
    let (status, joined) = redeem(&p, &zoe, made["token"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["role"], "member");
    assert_eq!(role_of(&p, &group, &zoe.actor).as_deref(), Some("member"));
}

#[tokio::test]
async fn join_requests_are_listed_approved_and_denied() {
    let p = FakeProvider::start().await.unwrap();
    let (alice, bob, group, _) = team(&p);
    p.configure_group(&group, |g| {
        g.tier = ofscp_core::wire::Tier::Public;
        g.join_policy = JoinPolicy::Request;
    });
    let carol = user(&p, "carol");
    let erin = user(&p, "erin");
    let join = format!("/api/groups/{}/join", group.as_str());
    let first = carol.send(&p, Method::POST, &join, None).await;
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.json);
    // Asking again is the same request.
    let again = carol.send(&p, Method::POST, &join, None).await;
    assert_eq!(again.status, StatusCode::ACCEPTED);
    assert_eq!(again.json["id"], first.json["id"]);
    erin.send(&p, Method::POST, &join, None).await;

    let list = format!("/api/groups/{}/requests", group.as_str());
    assert_eq!(
        bob.send(&p, Method::GET, &list, None).await.status,
        StatusCode::FORBIDDEN,
        "a plain member does not oversee requests"
    );
    let r = alice.send(&p, Method::GET, &list, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["items"].as_array().unwrap().len(), 2);

    let carols = first.json["id"].as_str().unwrap();
    let approve = format!("{list}/{carols}/approve");
    let r = alice.send(&p, Method::POST, &approve, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["role"], "member");
    assert_eq!(role_of(&p, &group, &carol.actor).as_deref(), Some("member"));
    // Answered: no longer pending.
    let r = alice.send(&p, Method::POST, &approve, None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let erins = p.join_requests(&group)[0].id.clone();
    let deny = format!("{list}/{}/deny", erins.as_str());
    let r = alice.send(&p, Method::POST, &deny, None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(p.join_requests(&group).is_empty());
    assert_eq!(role_of(&p, &group, &erin.actor), None);
}

#[tokio::test]
async fn a_request_is_not_approved_once_the_group_is_invite_only() {
    let p = FakeProvider::start().await.unwrap();
    let (alice, _, group, _) = team(&p);
    p.configure_group(&group, |g| {
        g.tier = ofscp_core::wire::Tier::Public;
        g.join_policy = JoinPolicy::Request;
    });
    let carol = user(&p, "carol");
    let join = format!("/api/groups/{}/join", group.as_str());
    let asked = carol.send(&p, Method::POST, &join, None).await;
    p.configure_group(&group, |g| g.join_policy = JoinPolicy::Invite);
    let approve = format!(
        "/api/groups/{}/requests/{}/approve",
        group.as_str(),
        asked.json["id"].as_str().unwrap()
    );
    let r = alice.send(&p, Method::POST, &approve, None).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.json);
    // Nor may anyone ask now.
    let erin = user(&p, "erin");
    let r = erin.send(&p, Method::POST, &join, None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}
