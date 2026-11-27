//! Joining (§5.6, §5.7): invite links to groups here and on other
//! providers, asking to join, and what a sync makes of both.

mod manage;

use std::sync::Arc;

use postrinch_ofscp::Endpoint;
use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::wire::{InviteCreateRequest, JoinPolicy, Tier};
use postrinch_ofscp::ofscp_core::{LocalId, ObjectUri};
use postrinch_ofscp_fake::FakeProvider;
use postrinch_social::{InviteLink, JoinAnswer, SocialError};
use postrinch_store::GroupFound;

use crate::common::{Home, PASSWORD, channel_uri, group_uri, home};

/// `handle`, a new user of `provider`, signed in with a client of their
/// own.
pub async fn signed_in(h: &Home, provider: &FakeProvider, handle: &str) -> Client {
    provider.add_user(handle, PASSWORD);
    let endpoint = Endpoint::new(provider.authority());
    let identity = h
        .transport
        .provider(endpoint.clone())
        .sign_in(handle, PASSWORD, "laptop")
        .await
        .expect("signs in");
    h.transport.client(Arc::new(identity), endpoint)
}

/// The link to an invite `by` makes to `group`, as `request` says.
pub async fn invite_link(
    by: &Client,
    provider: &FakeProvider,
    group: &LocalId,
    request: InviteCreateRequest,
) -> InviteLink {
    let made = by.create_invite(group, &request).await.expect("the invite");
    InviteLink::parse(&format!(
        "https://{}/invite/{}",
        provider.authority(),
        made.token
    ))
    .expect("a link")
}

/// The groups Ada's home lists her in, by URI.
async fn pointers(h: &Home) -> Vec<String> {
    let listed = h.phone.my_groups().await.expect("my groups").groups;
    listed.iter().map(|p| p.group.to_string()).collect()
}

fn found(h: &Home, uri: &str) -> Option<GroupFound> {
    h.social
        .store()
        .social_group(&h.account(), uri)
        .unwrap()
        .map(|g| g.found)
}

/// A group Dan owns on another provider, invite only, with a channel.
async fn away_group(h: &Home) -> (FakeProvider, Client, LocalId, LocalId) {
    let away = FakeProvider::start().await.unwrap();
    let dan = signed_in(h, &away, "dan").await;
    let group = away.create_group(&away.actor("dan"), "Away");
    away.configure_group(&group, |g| g.join_policy = JoinPolicy::Invite);
    let channel = away.create_channel(&group, "general");
    (away, dan, group, channel)
}

#[tokio::test]
async fn an_invite_link_joins_a_group_on_another_provider() {
    let h = home().await;
    h.add().await;
    let (away, dan, group, channel) = away_group(&h).await;
    away.post_as(&away.actor("dan"), &group, &channel, "welcome");
    let request = InviteCreateRequest {
        channel_id: Some(channel.clone()),
        ..InviteCreateRequest::default()
    };
    let link = invite_link(&dan, &away, &group, request).await;
    assert_eq!(link.provider(), away.authority().to_string());

    let joined = h
        .social
        .redeem_invite(&h.account(), &link)
        .await
        .expect("joined");
    let uri = group_uri(&away, &group);
    let place = channel_uri(&away, &group, &channel);
    assert_eq!(joined.group, uri);
    assert_eq!(joined.channel.as_deref(), Some(place.as_str()));
    assert_eq!(joined.role, "member");
    assert_eq!(
        away.roles(&group).get(&h.ada).map(String::as_str),
        Some("member")
    );
    assert_eq!(pointers(&h).await, [uri.clone()], "listed at home");
    assert_eq!(found(&h, &uri), Some(GroupFound::Member));
    assert_eq!(h.messages(&place).len(), 1, "its channel is synced");
    assert_eq!(away.invites(&group)[0].uses, 1);
}

#[tokio::test]
async fn redeeming_again_keeps_the_role_and_spends_nothing() {
    let h = home().await;
    h.add().await;
    let (away, dan, group, _) = away_group(&h).await;
    let request = InviteCreateRequest {
        max_uses: Some(5),
        ..InviteCreateRequest::default()
    };
    let link = invite_link(&dan, &away, &group, request).await;
    h.social.redeem_invite(&h.account(), &link).await.unwrap();
    let again = h.social.redeem_invite(&h.account(), &link).await.unwrap();
    assert_eq!(again.role, "member");
    assert_eq!(away.invites(&group)[0].uses, 1);
}

#[tokio::test]
async fn an_invite_spent_unknown_or_revoked_is_refused_with_the_reason() {
    let h = home().await;
    h.add().await;
    let (away, dan, group, _) = away_group(&h).await;
    let request = InviteCreateRequest {
        max_uses: Some(1),
        ..InviteCreateRequest::default()
    };
    let spent = invite_link(&dan, &away, &group, request).await;
    let erin = signed_in(&h, &away, "erin").await;
    erin.redeem_invite(spent.url().rsplit('/').next().unwrap())
        .await
        .expect("Erin takes the only use");
    let refused = h.social.redeem_invite(&h.account(), &spent).await;
    assert!(
        matches!(refused, Err(SocialError::InviteUsedUp)),
        "{refused:?}"
    );

    let unknown = InviteLink::parse(&format!("https://{}/invite/nope", away.authority())).unwrap();
    let refused = h.social.redeem_invite(&h.account(), &unknown).await;
    assert!(
        matches!(refused, Err(SocialError::InviteNotValid)),
        "{refused:?}"
    );

    let revoked = invite_link(&dan, &away, &group, InviteCreateRequest::default()).await;
    let id = away.invites(&group).last().unwrap().id.clone();
    dan.revoke_invite(&group, &id).await.unwrap();
    let refused = h.social.redeem_invite(&h.account(), &revoked).await;
    assert!(
        matches!(refused, Err(SocialError::InviteNotValid)),
        "{refused:?}"
    );
    assert_eq!(found(&h, &group_uri(&away, &group)), None, "nothing kept");
}

#[tokio::test]
async fn a_group_elsewhere_whose_pointer_home_lost_is_listed_again() {
    let h = home().await;
    h.add().await;
    let (away, dan, group, channel) = away_group(&h).await;
    let link = invite_link(&dan, &away, &group, InviteCreateRequest::default()).await;
    h.social.redeem_invite(&h.account(), &link).await.unwrap();
    let uri = group_uri(&away, &group);
    h.phone
        .delete_group_pointer(&ObjectUri::canonicalize(&uri).unwrap())
        .await
        .unwrap();
    assert!(pointers(&h).await.is_empty());

    h.social.sync(&h.account()).await.expect("sync");
    assert_eq!(pointers(&h).await, [uri.clone()], "written again");
    assert_eq!(found(&h, &uri), Some(GroupFound::Member));
    let place = channel_uri(&away, &group, &channel);
    assert!(
        h.social
            .store()
            .social_place(&h.account(), &place)
            .unwrap()
            .is_some()
    );

    // Once the group's provider says Ada is out, it is forgotten. She
    // leaves from another device, whose pointer delete never reached home.
    let tablet = h
        .transport
        .provider(Endpoint::new(h.provider.authority()))
        .sign_in("ada", PASSWORD, "tablet")
        .await
        .expect("the tablet signs in");
    let tablet = h
        .transport
        .client(Arc::new(tablet), Endpoint::new(away.authority()));
    tablet.leave(&group).await.expect("left");
    h.phone
        .delete_group_pointer(&ObjectUri::canonicalize(&uri).unwrap())
        .await
        .unwrap();
    h.social.sync(&h.account()).await.expect("sync");
    assert_eq!(found(&h, &uri), None);
    assert!(pointers(&h).await.is_empty());
}

/// Bob's group here, public, taking members as `policy` says.
async fn bobs_group(h: &Home, policy: JoinPolicy) -> (Client, LocalId, LocalId) {
    let bob = signed_in(h, &h.provider, "bob").await;
    let group = h.provider.create_group(&h.provider.actor("bob"), "Bob's");
    h.provider.configure_group(&group, |g| {
        g.tier = Tier::Public;
        g.join_policy = policy;
    });
    let channel = h.provider.create_channel(&group, "general");
    (bob, group, channel)
}

#[tokio::test]
async fn an_open_group_lets_ada_in_at_once() {
    let h = home().await;
    h.add().await;
    let (_, group, channel) = bobs_group(&h, JoinPolicy::Open).await;
    let uri = group_uri(&h.provider, &group);
    let answer = h.social.join_group(&h.account(), &uri).await.unwrap();
    let JoinAnswer::Joined(joined) = answer else {
        panic!("joined: {answer:?}");
    };
    assert_eq!(
        (joined.group.as_str(), joined.role.as_str()),
        (uri.as_str(), "member")
    );
    assert_eq!(found(&h, &uri), Some(GroupFound::Member));
    let place = channel_uri(&h.provider, &group, &channel);
    assert!(
        h.social
            .store()
            .social_place(&h.account(), &place)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn asking_waits_and_a_sync_finds_ada_let_in() {
    let h = home().await;
    h.add().await;
    let (bob, group, channel) = bobs_group(&h, JoinPolicy::Request).await;
    let uri = group_uri(&h.provider, &group);
    for _ in 0..2 {
        let answer = h.social.join_group(&h.account(), &uri).await.unwrap();
        assert!(matches!(answer, JoinAnswer::Asked { .. }), "{answer:?}");
    }
    assert_eq!(h.provider.join_requests(&group).len(), 1, "one request");
    assert_eq!(found(&h, &uri), Some(GroupFound::Asked));
    // A sync while it waits keeps it asked.
    h.social.sync(&h.account()).await.unwrap();
    assert_eq!(found(&h, &uri), Some(GroupFound::Asked));

    let request = h.provider.join_requests(&group)[0].id.clone();
    bob.approve_join_request(&group, &request).await.unwrap();
    h.social.sync(&h.account()).await.unwrap();
    assert_eq!(found(&h, &uri), Some(GroupFound::Member));
    let place = channel_uri(&h.provider, &group, &channel);
    assert!(
        h.social
            .store()
            .social_place(&h.account(), &place)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn an_invite_only_group_refuses_asking() {
    let h = home().await;
    h.add().await;
    let (_, group, _) = bobs_group(&h, JoinPolicy::Invite).await;
    let uri = group_uri(&h.provider, &group);
    let refused = h.social.join_group(&h.account(), &uri).await;
    assert!(
        matches!(&refused, Err(SocialError::InviteOnly { group }) if *group == uri),
        "{refused:?}"
    );
    assert_eq!(found(&h, &uri), None);
    let not_a_group = h
        .social
        .join_group(&h.account(), "https://x.example/")
        .await;
    assert!(matches!(not_a_group, Err(SocialError::NotAGroup(_))));
}

#[tokio::test]
async fn a_followed_channels_group_is_kept_to_join_and_forgotten_with_it() {
    use postrinch_ofscp::ofscp_core::ObjectRef;
    use postrinch_ofscp::ofscp_core::wire::FollowCreateRequest;

    let h = home().await;
    let (_, group, channel) = bobs_group(&h, JoinPolicy::Open).await;
    h.provider
        .configure_channel(&channel, |c| c.tier = Tier::Public);
    h.phone
        .follow(&FollowCreateRequest {
            channel: ObjectRef::Local(channel.clone()),
            group_id: Some(ObjectRef::Local(group.clone())),
        })
        .await
        .expect("the follow");
    h.add().await;
    let uri = group_uri(&h.provider, &group);
    assert_eq!(found(&h, &uri), Some(GroupFound::Followed));

    h.phone
        .unfollow(&ObjectRef::Local(channel.clone()))
        .await
        .expect("unfollowed");
    h.social.sync(&h.account()).await.unwrap();
    assert_eq!(found(&h, &uri), None);
}
