//! Edits, deletes, reactions, read markers and follows, sent and kept.

use postrinch_ofscp::ofscp_core::ObjectRef;
use postrinch_store::SocialChange;

use crate::common::{channel_uri, home};

use super::{chat, kept_sent, outbox, queue, send, text};

#[tokio::test]
async fn edits_reactions_and_deletes_are_sent_and_kept() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    let bobs = h.provider.post_as(&bob, &group, &channel, "Bob's");
    h.add().await;
    queue(&h, "c1", &place, &chat("first draft", None));
    assert_eq!(send(&h).await.sent, 1);
    let mine = kept_sent(&h, &place, "c1").id;

    let react = |message: &str, on: bool| SocialChange::React {
        message: message.into(),
        key: "👍".into(),
        on,
    };
    let edit = SocialChange::Edit {
        message: mine.clone(),
        content: text("second draft"),
    };
    queue(&h, "e1", &place, &edit);
    queue(&h, "r1", &place, &react(bobs.id.as_str(), true));
    queue(&h, "r2", &place, &react(&mine, true));
    // Adding it again changes nothing at the provider.
    queue(&h, "r3", &place, &react(&mine, true));
    let sent = send(&h).await;
    assert_eq!((sent.sent, sent.failed, sent.refused), (4, 0, 0));

    let at_provider = h.provider.messages(&group, &channel);
    let edited = at_provider.iter().find(|m| m.id.as_str() == mine).unwrap();
    assert_eq!(edited.content.as_ref().unwrap().text, "second draft");
    assert!(edited.edited_at.is_some());
    let kept = kept_sent(&h, &place, "c1");
    assert_eq!(kept.message["content"]["text"], "second draft");
    assert_eq!(kept.reactions.len(), 1);
    assert_eq!((kept.reactions[0].count, kept.reactions[0].mine), (1, true));
    let bobs_kept = h
        .messages(&place)
        .into_iter()
        .find(|m| m.id == bobs.id.as_str())
        .unwrap();
    assert_eq!(bobs_kept.reactions[0].key, "👍");
    // Reactions are put by key (§7.1 `PUT …/reactions/{key}`).
    let reacted = format!("/messages/{mine}/reactions/");
    let puts = h
        .provider
        .requests()
        .into_iter()
        .filter(|r| r.method == "PUT" && r.path.contains(&reacted))
        .count();
    assert_eq!(puts, 2);

    // Taken back, then the message deleted, twice.
    queue(&h, "r4", &place, &react(&mine, false));
    queue(
        &h,
        "d1",
        &place,
        &SocialChange::Delete {
            message: mine.clone(),
        },
    );
    queue(
        &h,
        "d2",
        &place,
        &SocialChange::Delete {
            message: mine.clone(),
        },
    );
    assert_eq!(send(&h).await.sent, 3);
    let kept = kept_sent(&h, &place, "c1");
    assert!(kept.deleted && kept.reactions.is_empty());
    let at_provider = h.provider.messages(&group, &channel);
    let gone = at_provider.iter().find(|m| m.id.as_str() == mine).unwrap();
    assert!(gone.deleted_at.is_some());
    assert!(outbox(&h).is_empty());
}

#[tokio::test]
async fn a_reaction_the_provider_has_is_done_though_reading_back_fails() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    let bobs = h.provider.post_as(&bob, &group, &channel, "Bob's");
    h.add().await;

    // The reaction is put; reading the message's reactions back fails.
    h.provider.faults().fail_reads_ending(Some("/reactions"));
    let react = SocialChange::React {
        message: bobs.id.to_string(),
        key: "👍".into(),
        on: true,
    };
    queue(&h, "r1", &place, &react);
    queue(&h, "c1", &place, &chat("after it", None));
    let sent = send(&h).await;
    assert_eq!((sent.sent, sent.failed, sent.refused), (2, 0, 0));
    assert!(outbox(&h).is_empty());
    let reacted = format!("/messages/{}/reactions/", bobs.id);
    let puts = h
        .provider
        .requests()
        .into_iter()
        .filter(|r| r.method == "PUT" && r.path.contains(&reacted))
        .count();
    assert_eq!(puts, 1, "put once, not again");
}

#[tokio::test]
async fn a_read_marker_moves_at_home() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.provider.post_as(&bob, &group, &channel, "one");
    let two = h.provider.post_as(&bob, &group, &channel, "two");
    h.add().await;

    let read = SocialChange::Read {
        message: two.id.to_string(),
    };
    queue(&h, "m1", &place, &read);
    assert_eq!(send(&h).await.sent, 1);
    let markers = h.phone.read_markers().await.unwrap();
    let marker = markers
        .scopes
        .iter()
        .find(|m| m.scope_id == channel)
        .expect("a marker for the channel");
    assert_eq!(marker.last_read_message_id.as_ref(), Some(&two.id));
}

#[tokio::test]
async fn following_and_unfollowing_are_sent_and_kept() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;
    let followed = |h: &crate::common::Home| {
        h.social
            .store()
            .social_place(&h.account(), &place)
            .unwrap()
            .unwrap()
            .followed
    };
    assert!(!followed(&h));

    let follow = |on| SocialChange::Follow {
        group: Some(crate::common::group_uri(&h.provider, &group)),
        on,
    };
    queue(&h, "f1", &place, &follow(true));
    // Following twice is one follow (§7.6).
    queue(&h, "f2", &place, &follow(true));
    assert_eq!(send(&h).await.sent, 2);
    let follows = h.phone.follows().await.unwrap().follows;
    assert_eq!(follows.len(), 1);
    // A channel of home is named by its local id.
    assert_eq!(follows[0].channel, ObjectRef::Local(channel.clone()));
    assert!(followed(&h));

    queue(&h, "f3", &place, &follow(false));
    assert_eq!(send(&h).await.sent, 1);
    assert!(h.phone.follows().await.unwrap().follows.is_empty());
    assert!(!followed(&h));
}
