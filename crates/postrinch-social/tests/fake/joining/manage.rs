//! A group's managers: invites made, listed and revoked, and requests to
//! join answered, through the engine.

use postrinch_ofscp::ofscp_core::wire::{JoinPolicy, Tier};
use postrinch_social::{InviteLink, MakeInvite, SocialError};

use super::signed_in;
use crate::common::{channel_uri, group_uri, home};

#[tokio::test]
async fn a_manager_makes_lists_and_revokes_invites() {
    let h = home().await;
    let (_, group, channel) = h.bob_with_group();
    h.provider.add_member(&group, &h.ada, "admin");
    h.add().await;
    let uri = group_uri(&h.provider, &group);
    let place = channel_uri(&h.provider, &group, &channel);

    let expires = h.provider.now().as_millisecond() + 86_400_000;
    let made = h
        .social
        .make_invite(
            &h.account(),
            &uri,
            MakeInvite {
                channel: Some(place.clone()),
                role: None,
                max_uses: Some(10),
                expires_at: Some(expires),
            },
        )
        .await
        .expect("made");
    assert_eq!(made.link.provider(), h.provider.authority().to_string());
    assert_eq!(InviteLink::parse(&made.link.url()), Some(made.link.clone()));
    assert_eq!(made.invite.channel.as_deref(), Some(place.as_str()));
    assert_eq!(made.invite.max_uses, Some(10));
    assert_eq!(made.invite.expires_at, Some(expires));
    assert_eq!(made.invite.created_by, h.account());

    // A channel of the same id in another group is not this group's.
    let elsewhere = place.replace(group.as_str(), "grp_other");
    let wrong = MakeInvite {
        channel: Some(elsewhere),
        ..MakeInvite::default()
    };
    let refused = h.social.make_invite(&h.account(), &uri, wrong).await;
    assert!(
        matches!(refused, Err(SocialError::NotAChannel(_))),
        "{refused:?}"
    );

    let listed = h.social.group_invites(&h.account(), &uri).await.unwrap();
    assert_eq!(listed, std::slice::from_ref(&made.invite));

    h.social
        .revoke_invite(&h.account(), &uri, &made.invite.id)
        .await
        .expect("revoked");
    assert!(
        h.social
            .group_invites(&h.account(), &uri)
            .await
            .unwrap()
            .is_empty()
    );
    let carol = signed_in(&h, &h.provider, "carol").await;
    let token = made.link.url().rsplit('/').next().unwrap().to_owned();
    assert!(
        carol.redeem_invite(&token).await.is_err(),
        "the link is dead"
    );
}

#[tokio::test]
async fn invites_beyond_a_members_rights_are_refused() {
    let h = home().await;
    let (_, group, _) = h.bob_with_group();
    h.add().await;
    let uri = group_uri(&h.provider, &group);
    let refused = h
        .social
        .make_invite(&h.account(), &uri, MakeInvite::default())
        .await;
    let status = match &refused {
        Err(SocialError::Transport(e)) => e.problem().map(|p| p.status),
        _ => None,
    };
    assert_eq!(status, Some(403), "{refused:?}");
    let refused = h.social.group_invites(&h.account(), &uri).await;
    assert!(refused.is_err());
    let bad = MakeInvite {
        channel: Some("not a channel".to_owned()),
        ..MakeInvite::default()
    };
    let refused = h.social.make_invite(&h.account(), &uri, bad).await;
    assert!(matches!(refused, Err(SocialError::NotAChannel(_))));
}

#[tokio::test]
async fn managers_answer_requests_to_join() {
    let h = home().await;
    let (_, group, _) = h.bob_with_group();
    h.provider.add_member(&group, &h.ada, "admin");
    h.provider.configure_group(&group, |g| {
        g.tier = Tier::Public;
        g.join_policy = JoinPolicy::Request;
    });
    h.add().await;
    let uri = group_uri(&h.provider, &group);
    let carol = signed_in(&h, &h.provider, "carol").await;
    let erin = signed_in(&h, &h.provider, "erin").await;
    carol.join(&group).await.unwrap();
    erin.join(&group).await.unwrap();

    let pending = h.social.join_requests(&h.account(), &uri).await.unwrap();
    let users: Vec<&str> = pending.iter().map(|r| r.user.as_str()).collect();
    let (carol_actor, erin_actor) = (h.provider.actor("carol"), h.provider.actor("erin"));
    assert_eq!(users.len(), 2);
    assert!(users.contains(&carol_actor.to_string().as_str()));
    let id_of = |who: &str| pending.iter().find(|r| r.user == who).unwrap().id.clone();

    h.social
        .approve_join_request(&h.account(), &uri, &id_of(&carol_actor.to_string()))
        .await
        .expect("approved");
    h.social
        .deny_join_request(&h.account(), &uri, &id_of(&erin_actor.to_string()))
        .await
        .expect("denied");
    let roles = h.provider.roles(&group);
    assert_eq!(roles.get(&carol_actor).map(String::as_str), Some("member"));
    assert_eq!(roles.get(&erin_actor), None);
    assert!(
        h.social
            .join_requests(&h.account(), &uri)
            .await
            .unwrap()
            .is_empty()
    );

    // A group gone invite only takes no more approvals.
    let frank = signed_in(&h, &h.provider, "frank").await;
    frank.join(&group).await.unwrap();
    h.provider
        .configure_group(&group, |g| g.join_policy = JoinPolicy::Invite);
    let id = h.social.join_requests(&h.account(), &uri).await.unwrap()[0]
        .id
        .clone();
    let refused = h.social.approve_join_request(&h.account(), &uri, &id).await;
    let status = match &refused {
        Err(SocialError::Transport(e)) => e.problem().map(|p| p.status),
        _ => None,
    };
    assert_eq!(status, Some(409), "{refused:?}");
}
