//! What a sync makes of groups joined or asked that home does not list,
//! when their providers answer less than they might: a group not read
//! after joining, a provider that keeps `membership` to itself (§5.5), one
//! that refuses the signature, and a group closed to outsiders while a
//! request waits (§5.7).

use postrinch_ofscp::ofscp_core::wire::{InviteCreateRequest, JoinPolicy, Tier};
use postrinch_social::JoinAnswer;
use postrinch_store::GroupFound;

use super::{away_group, bobs_group, drop_pointer, found, invite_link, leave_elsewhere, pointers};
use crate::common::{group_uri, home};

#[tokio::test]
async fn a_group_not_read_after_redeeming_is_kept_and_read_later() {
    let h = home().await;
    h.add().await;
    let (away, dan, group, _) = away_group(&h).await;
    let request = InviteCreateRequest {
        max_uses: Some(1),
        ..InviteCreateRequest::default()
    };
    let link = invite_link(&dan, &away, &group, request).await;
    let path = format!("/api/groups/{}", group.as_str());
    away.faults().fail_reads_ending(Some(&path));

    // The only use is spent: the join must not be lost to the failed read.
    let joined = h.social.redeem_invite(&h.account(), &link).await;
    let uri = group_uri(&away, &group);
    assert_eq!(joined.expect("joined").group, uri);
    assert_eq!(found(&h, &uri), Some(GroupFound::Member));

    away.faults().fail_reads_ending(None);
    h.social.sync(&h.account()).await.expect("sync");
    assert_eq!(pointers(&h).await, std::slice::from_ref(&uri));
    let kept = h.social.store().social_group(&h.account(), &uri).unwrap();
    assert_eq!(kept.unwrap().group["name"], "Away", "read at last");
}

#[tokio::test]
async fn a_provider_keeping_membership_to_itself_is_asked_its_member_list() {
    let h = home().await;
    h.add().await;
    let (away, dan, group, _) = away_group(&h).await;
    away.configure_group(&group, |g| g.tier = Tier::Public);
    let link = invite_link(&dan, &away, &group, InviteCreateRequest::default()).await;
    h.social.redeem_invite(&h.account(), &link).await.unwrap();
    let uri = group_uri(&away, &group);
    drop_pointer(&h, &uri).await;
    away.faults().omit_membership(true);

    h.social.sync(&h.account()).await.expect("sync");
    assert_eq!(
        pointers(&h).await,
        std::slice::from_ref(&uri),
        "still a member"
    );
    assert_eq!(found(&h, &uri), Some(GroupFound::Member));

    // Out of the member list, out of the group.
    leave_elsewhere(&h, &away, &group).await;
    h.social.sync(&h.account()).await.expect("sync");
    assert_eq!(found(&h, &uri), None);
}

#[tokio::test]
async fn a_group_whose_provider_refuses_the_signature_is_left_as_kept() {
    let h = home().await;
    h.add().await;
    let (away, dan, group, _) = away_group(&h).await;
    let link = invite_link(&dan, &away, &group, InviteCreateRequest::default()).await;
    h.social.redeem_invite(&h.account(), &link).await.unwrap();
    let uri = group_uri(&away, &group);
    drop_pointer(&h, &uri).await;

    away.faults().refuse_signatures(true);
    h.social.sync(&h.account()).await.expect("sync");
    assert_eq!(found(&h, &uri), Some(GroupFound::Member), "spared");
    away.faults().refuse_signatures(false);
    h.social.sync(&h.account()).await.expect("sync");
    assert_eq!(pointers(&h).await, std::slice::from_ref(&uri));
}

#[tokio::test]
async fn a_request_outlives_its_group_closing_to_outsiders() {
    let h = home().await;
    h.add().await;
    let (bob, group, _) = bobs_group(&h, JoinPolicy::Request).await;
    let uri = group_uri(&h.provider, &group);
    let answer = h.social.join_group(&h.account(), &uri).await.unwrap();
    assert!(matches!(answer, JoinAnswer::Asked { .. }), "{answer:?}");

    // Now only members may read it (403), but the request is kept (§5.7).
    h.provider.configure_group(&group, |g| g.tier = Tier::Group);
    h.social.sync(&h.account()).await.unwrap();
    assert_eq!(found(&h, &uri), Some(GroupFound::Asked));

    let request = h.provider.join_requests(&group)[0].id.clone();
    bob.approve_join_request(&group, &request).await.unwrap();
    h.social.sync(&h.account()).await.unwrap();
    assert_eq!(found(&h, &uri), Some(GroupFound::Member));

    // A group that is gone (404) takes its request with it.
    let other = h.provider.create_group(&h.provider.actor("bob"), "Other");
    h.provider.configure_group(&other, |g| {
        g.tier = Tier::Public;
        g.join_policy = JoinPolicy::Request;
    });
    let other_uri = group_uri(&h.provider, &other);
    h.social.join_group(&h.account(), &other_uri).await.unwrap();
    h.provider.remove_group(&other);
    h.social.sync(&h.account()).await.unwrap();
    assert_eq!(found(&h, &other_uri), None);
}
