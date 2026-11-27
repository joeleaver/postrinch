//! The first sync: groups, places and each place's newest page, read or
//! unread as the read markers say.

use std::sync::Arc;

use postrinch_ofscp::Endpoint;
use postrinch_ofscp::ofscp_core::wire::{
    DmMessageCreate, FollowCreateRequest, GroupPointerRequest, NotificationMode, NotificationScope,
    NotificationSetting, ReadMarkerUpdate, ReadMarkersUpdateRequest, Tier,
};
use postrinch_ofscp::ofscp_core::{ObjectRef, ObjectUri, dm_id};
use postrinch_ofscp_fake::{Authentication, FakeProvider};
use postrinch_store::{GroupFound, PlaceKind};
use serde_json::json;

use crate::common::{PASSWORD, channel_uri, group_uri, home};

#[tokio::test]
async fn the_first_sync_keeps_groups_channels_and_newest_pages() {
    let h = home().await;
    let (bob, group, general) = h.bob_with_group();
    let random = h.provider.create_channel(&group, "random");
    let posts: Vec<_> = ["one", "two", "three"]
        .iter()
        .map(|text| h.provider.post_as(&bob, &group, &general, text))
        .collect();
    h.provider.post_as(&bob, &group, &random, "elsewhere");
    let read = ReadMarkerUpdate {
        scope_id: general.clone(),
        last_read_message_id: posts[1].id.clone(),
    };
    h.phone
        .update_read_markers(&ReadMarkersUpdateRequest {
            markers: vec![read],
        })
        .await
        .expect("the marker");
    let setting = NotificationSetting {
        scope_type: NotificationScope::Group,
        scope_id: group.clone(),
        mode: NotificationMode::Mentions,
    };
    h.provider.set_notification_settings(&h.ada, vec![setting]);

    let synced = h.add().await.synced;
    assert_eq!((synced.groups, synced.places, synced.skipped), (1, 2, 0));
    assert_eq!(synced.messages, 4);

    let store = h.social.store();
    let account = store.social_account(&h.account()).unwrap().unwrap();
    assert_eq!(account.profile.unwrap()["handle"], "ada");
    assert_eq!(account.settings.unwrap()["prefs"][0]["mode"], "mentions");

    let kept = store
        .social_group(&h.account(), &group_uri(&h.provider, &group))
        .unwrap()
        .expect("the group");
    assert_eq!(kept.role.as_deref(), Some("member"));
    assert_eq!(kept.found, GroupFound::Member);
    assert_eq!(kept.group["name"], "Bob's group");

    let place_id = channel_uri(&h.provider, &group, &general);
    let place = store
        .social_place(&h.account(), &place_id)
        .unwrap()
        .unwrap();
    assert_eq!(place.kind, PlaceKind::Channel);
    assert_eq!(place.group, Some(kept.uri.clone()));
    assert!(!place.followed);
    assert_eq!(place.read_marker.as_deref(), Some(posts[1].id.as_str()));
    assert!(place.cursors.complete, "one page holds it all");
    let unread: Vec<_> = h.messages(&place_id).iter().map(|m| m.unread).collect();
    assert_eq!(unread, [false, false, true], "after the marker");

    // The provider lists every scope; nothing read in this one (§7.7).
    let random_id = channel_uri(&h.provider, &group, &random);
    let [only] = &h.messages(&random_id)[..] else {
        panic!("one message");
    };
    assert!(only.unread);
    assert_eq!(only.author, bob.to_string());

    let provider = store
        .social_provider(h.provider.authority().as_str())
        .unwrap()
        .expect("the provider");
    assert_eq!(provider.clock_offset_ms, 0);
    assert_eq!(
        provider.discovery["provider"]["software"]["name"],
        "postrinch-ofscp-fake"
    );
}

#[tokio::test]
async fn followed_channels_and_conversations_are_kept() {
    let h = home().await;
    // A public channel Ada follows without joining its group.
    let carol = h.provider.add_user("carol", PASSWORD);
    let group = h.provider.create_group(&carol, "Public");
    h.provider
        .configure_group(&group, |g| g.tier = Tier::Public);
    let channel = h.provider.create_channel(&group, "news");
    h.provider
        .configure_channel(&channel, |c| c.tier = Tier::Public);
    h.provider.post_as(&carol, &group, &channel, "news!");
    h.phone
        .follow(&FollowCreateRequest {
            channel: ObjectRef::Local(channel.clone()),
            group_id: Some(ObjectRef::Local(group.clone())),
        })
        .await
        .expect("the follow");

    // A private conversation with Carol.
    let endpoint = Endpoint::new(h.provider.authority());
    let identity = h
        .transport
        .provider(endpoint.clone())
        .sign_in("carol", PASSWORD, "carol's")
        .await
        .unwrap();
    let carol_client = h.transport.client(Arc::new(identity), endpoint);
    let dm = dm_id(&carol, &h.ada);
    let message: DmMessageCreate = serde_json::from_value(json!({
        "clientMessageId": "c1",
        "to": h.ada.to_string(),
        "content": { "text": "psst", "mime": "text/plain" },
    }))
    .unwrap();
    carol_client.send_dm(&dm, &message).await.expect("the DM");

    let synced = h.add().await.synced;
    assert_eq!((synced.groups, synced.places), (0, 2));
    let store = h.social.store();
    let followed = store
        .social_place(&h.account(), &channel_uri(&h.provider, &group, &channel))
        .unwrap()
        .expect("the followed channel");
    assert!(followed.followed);
    assert_eq!(followed.group, Some(group_uri(&h.provider, &group)));
    // Its group is kept as followed, for All channels to offer joining.
    let groups = store.social_groups(&h.account()).unwrap();
    let found: Vec<_> = groups.iter().map(|g| (g.uri.clone(), g.found)).collect();
    assert_eq!(
        found,
        [(group_uri(&h.provider, &group), GroupFound::Followed)]
    );

    let conversation = store
        .social_place(&h.account(), dm.as_str())
        .unwrap()
        .expect("the conversation");
    assert_eq!(conversation.kind, PlaceKind::Conversation);
    let [psst] = &h.messages(dm.as_str())[..] else {
        panic!("one message");
    };
    assert_eq!(psst.author, carol.to_string());
}

#[tokio::test]
async fn a_channel_on_another_provider_is_read_there_with_the_key_from_home() {
    let h = home().await;
    let away = FakeProvider::start().await.unwrap();
    let dan = away.add_user("dan", PASSWORD);
    let group = away.create_group(&dan, "Away");
    let channel = away.create_channel(&group, "general");
    away.add_member(&group, &h.ada, "member");
    away.post_as(&dan, &group, &channel, "first");
    let uri = ObjectUri::canonicalize(&group_uri(&away, &group)).unwrap();
    h.phone
        .put_group_pointer(&uri, &GroupPointerRequest { role: None })
        .await
        .expect("the pointer");

    let key_id = h.add().await.account.key_id.unwrap();
    let place = channel_uri(&away, &group, &channel);
    let first = h.messages(&place);
    assert_eq!(first.len(), 1);
    assert!(!first[0].unread, "no marker there: read when first seen");
    let messages = format!("/api/groups/{group}/channels/{channel}/messages");
    let request = &crate::common::requests_to(&away, &messages)[0];
    assert_eq!(
        request.authentication,
        Authentication::Verified {
            actor: h.ada.clone(),
            key_id,
        }
    );
    assert!(
        h.social
            .store()
            .social_provider(away.authority().as_str())
            .unwrap()
            .is_some(),
        "the other provider is kept too"
    );

    // From then on the place reads by its own marker.
    away.post_as(&dan, &group, &channel, "second");
    let synced = h.social.sync(&h.account()).await.expect("sync again");
    assert_eq!(synced.places, 1);
    let unread: Vec<_> = h.messages(&place).iter().map(|m| m.unread).collect();
    assert_eq!(unread, [false, true]);
}

#[tokio::test]
async fn a_group_no_longer_readable_is_left_out() {
    let h = home().await;
    let away = FakeProvider::start().await.unwrap();
    let dan = away.add_user("dan", PASSWORD);
    // Ada was never let in: the pointer is stale.
    let group = away.create_group(&dan, "Closed");
    let uri = ObjectUri::canonicalize(&group_uri(&away, &group)).unwrap();
    h.phone
        .put_group_pointer(&uri, &GroupPointerRequest { role: None })
        .await
        .unwrap();
    let synced = h.add().await.synced;
    assert_eq!((synced.groups, synced.skipped), (0, 1));
    let pointers = h.phone.my_groups().await.unwrap().groups;
    assert!(pointers.is_empty(), "the stale pointer is dropped at home");
}

#[tokio::test]
async fn a_group_left_and_a_channel_unfollowed_are_forgotten() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    h.provider.post_as(&bob, &group, &channel, "hello");
    let news_group = h.provider.create_group(&bob, "Public");
    h.provider
        .configure_group(&news_group, |g| g.tier = Tier::Public);
    let news = h.provider.create_channel(&news_group, "news");
    h.provider
        .configure_channel(&news, |c| c.tier = Tier::Public);
    let followed = ObjectRef::Local(news.clone());
    h.phone
        .follow(&FollowCreateRequest {
            channel: followed.clone(),
            group_id: Some(ObjectRef::Local(news_group.clone())),
        })
        .await
        .unwrap();
    assert_eq!(h.add().await.synced.places, 2);

    h.phone.leave(&group).await.expect("left");
    h.phone.unfollow(&followed).await.expect("unfollowed");
    let synced = h.social.sync(&h.account()).await.unwrap();
    assert_eq!((synced.groups, synced.places), (0, 0));
    let store = h.social.store();
    assert!(store.social_groups(&h.account()).unwrap().is_empty());
    assert!(store.social_places(&h.account()).unwrap().is_empty());
    assert!(
        h.messages(&channel_uri(&h.provider, &group, &channel))
            .is_empty()
    );
}

#[tokio::test]
async fn syncing_again_refreshes_without_doubling() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    h.provider.post_as(&bob, &group, &channel, "one");
    h.add().await;
    h.provider.post_as(&bob, &group, &channel, "two");
    let synced = h.social.sync(&h.account()).await.unwrap();
    assert_eq!((synced.groups, synced.places, synced.messages), (1, 1, 2));
    let place = channel_uri(&h.provider, &group, &channel);
    let kept = h.messages(&place);
    assert_eq!(kept.len(), 2);
    // Nothing read on the provider yet, so both are unread.
    assert_eq!(
        kept.iter().map(|m| m.unread).collect::<Vec<_>>(),
        [true, true]
    );
}

#[tokio::test]
async fn a_channel_still_followed_outlives_leaving_its_group() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    h.provider
        .configure_group(&group, |g| g.tier = Tier::Public);
    h.provider
        .configure_channel(&channel, |c| c.tier = Tier::Public);
    h.provider.post_as(&bob, &group, &channel, "hello");
    h.phone
        .follow(&FollowCreateRequest {
            channel: ObjectRef::Local(channel.clone()),
            group_id: Some(ObjectRef::Local(group.clone())),
        })
        .await
        .unwrap();
    h.add().await;

    h.phone.leave(&group).await.expect("left");
    let synced = h.social.sync(&h.account()).await.unwrap();
    assert_eq!((synced.groups, synced.places), (0, 1));
    let store = h.social.store();
    let groups = store.social_groups(&h.account()).unwrap();
    let found: Vec<_> = groups.iter().map(|g| (g.uri.clone(), g.found)).collect();
    assert_eq!(
        found,
        [(group_uri(&h.provider, &group), GroupFound::Followed)]
    );
    let place_id = channel_uri(&h.provider, &group, &channel);
    let place = store
        .social_place(&h.account(), &place_id)
        .unwrap()
        .expect("still followed");
    assert!(place.followed);
    assert_eq!(h.messages(&place_id).len(), 1);
}

#[tokio::test]
async fn a_conversation_with_someone_elsewhere_has_both_sides() {
    let h = home().await;
    let away = FakeProvider::start().await.unwrap();
    let erin = away.add_user("erin", PASSWORD);
    let endpoint = Endpoint::new(away.authority());
    let identity = h
        .transport
        .provider(endpoint.clone())
        .sign_in("erin", PASSWORD, "erin's")
        .await
        .unwrap();
    let erin_client = h.transport.client(Arc::new(identity), endpoint);
    let dm = dm_id(&erin, &h.ada);
    let to = |who: &postrinch_ofscp::ofscp_core::Actor, id: &str, text: &str| -> DmMessageCreate {
        serde_json::from_value(json!({
            "clientMessageId": id,
            "to": who.to_string(),
            "content": { "text": text, "mime": "text/plain" },
        }))
        .unwrap()
    };
    // Erin's message goes to Ada's provider; Ada's answer to Erin's.
    let at_home = h.transport.client(
        erin_client.identity().clone(),
        Endpoint::new(h.provider.authority()),
    );
    at_home
        .send_dm(&dm, &to(&h.ada, "e1", "hi Ada"))
        .await
        .expect("to Ada");
    let phone_away = h
        .transport
        .client(h.phone.identity().clone(), Endpoint::new(away.authority()));
    phone_away
        .send_dm(&dm, &to(&erin, "a1", "hi Erin"))
        .await
        .expect("to Erin");

    h.add().await;
    let kept = h.messages(dm.as_str());
    assert_eq!(kept.len(), 2);
    // Both were sent in the same instant, so find each by its text.
    let said = |text: &str| {
        kept.iter()
            .find(|m| m.message["content"]["text"] == text)
            .unwrap_or_else(|| panic!("{text:?} is kept"))
    };
    // Each provider numbers its own messages: the one held at Erin's is
    // kept under its URL there, so the two never collide.
    let (theirs, mine) = (said("hi Ada"), said("hi Erin"));
    let local = mine.message["id"].as_str().unwrap();
    assert_eq!(
        mine.id,
        format!("https://{}/api/dms/{dm}/messages/{local}", away.authority())
    );
    assert_eq!(theirs.id, theirs.message["id"].as_str().unwrap());
    assert!(theirs.unread && !mine.unread);
}
