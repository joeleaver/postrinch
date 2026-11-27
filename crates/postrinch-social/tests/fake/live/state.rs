//! Read markers both ways, two providers at once, and presence.

use postrinch_ofscp::ofscp_core::ObjectUri;
use postrinch_ofscp::ofscp_core::wire::{
    Availability, GroupPointerRequest, ReadMarkerUpdate, ReadMarkersUpdateRequest,
};
use postrinch_ofscp_fake::FakeProvider;
use postrinch_social::{LiveEvent, SessionStatus};
use postrinch_store::SocialAccountState;

use super::{is_new, live, post_each};
use crate::common::{Home, PASSWORD, channel_uri, group_uri, home};

/// Ada's phone reads `channel` up to `id`.
async fn phone_reads(h: &Home, channel: &postrinch_ofscp::ofscp_core::LocalId, id: &str) {
    let marker = ReadMarkerUpdate {
        scope_id: channel.clone(),
        last_read_message_id: postrinch_ofscp::ofscp_core::LocalId::new(id).unwrap(),
    };
    let request = ReadMarkersUpdateRequest {
        markers: vec![marker],
    };
    super::elsewhere(|| h.phone.update_read_markers(&request)).await;
}

#[tokio::test]
async fn reading_on_the_phone_reads_here_live_and_after_a_reconnect() {
    let h = home().await;
    let (bob, group, general) = h.bob_with_group();
    let ids = post_each(
        &h.provider,
        &bob,
        &group,
        &general,
        &["one", "two", "three"],
    )
    .await;
    let (_social, mut told) = live(&h, 1).await;
    let place = channel_uri(&h.provider, &group, &general);

    phone_reads(&h, &general, &ids[1]).await;
    let event = told
        .until("the marker", |e| matches!(e, LiveEvent::Read { .. }))
        .await;
    assert_eq!(
        event,
        LiveEvent::Read {
            account: h.account(),
            place: place.clone(),
            marker: ids[1].clone(),
        }
    );
    let unread: Vec<_> = h.messages(&place).iter().map(|m| m.unread).collect();
    assert_eq!(unread, [false, false, true]);

    // Read further while this computer is offline: the reconnect reads it.
    h.provider.faults().refuse_ws_auth(true);
    h.provider.faults().drop_sockets();
    told.until("the session ending", |e| {
        matches!(e, LiveEvent::Disconnected { .. })
    })
    .await;
    phone_reads(&h, &general, &ids[2]).await;
    h.provider.faults().refuse_ws_auth(false);
    told.caught_up(1).await;
    let unread: Vec<_> = h.messages(&place).iter().map(|m| m.unread).collect();
    assert_eq!(unread, [false, false, false]);
    let kept = h
        .social
        .store()
        .social_place(&h.account(), &place)
        .unwrap()
        .unwrap();
    assert_eq!(kept.read_marker.as_deref(), Some(ids[2].as_str()));
}

#[tokio::test]
async fn two_providers_each_get_a_session_and_keep_their_own_markers() {
    let h = home().await;
    let away = FakeProvider::start().await.unwrap();
    let dan = away.add_user("dan", PASSWORD);
    let group = away.create_group(&dan, "Away");
    let channel = away.create_channel(&group, "general");
    away.add_member(&group, &h.ada, "member");
    // In a second of its own: the marker is by time, in whole seconds.
    post_each(&away, &dan, &group, &channel, &["first"]).await;
    let uri = ObjectUri::canonicalize(&group_uri(&away, &group)).unwrap();
    h.phone
        .put_group_pointer(&uri, &GroupPointerRequest { role: None })
        .await
        .expect("the pointer");
    let (social, mut told) = live(&h, 2).await;

    let mut sessions = social.sessions(&h.account());
    sessions.sort_by(|a, b| a.provider.cmp(&b.provider));
    let mut expected = vec![
        SessionStatus {
            provider: h.provider.authority().to_string(),
            connected: true,
        },
        SessionStatus {
            provider: away.authority().to_string(),
            connected: true,
        },
    ];
    expected.sort_by(|a, b| a.provider.cmp(&b.provider));
    assert_eq!(sessions, expected);

    // There, the place's marker is its own: what was there when first
    // seen is read, and what comes later is not.
    let place = channel_uri(&away, &group, &channel);
    let later = away.post_as(&dan, &group, &channel, "later").id.to_string();
    told.until("the message there", |e| is_new(e, &later)).await;
    let unread: Vec<_> = h.messages(&place).iter().map(|m| m.unread).collect();
    assert_eq!(unread, [false, true]);

    // A 4003 from there is checked with home, which still takes the key:
    // the session tries again, and the account stays active.
    away.faults().close_sockets_revoked();
    told.until("the session there ending", |e| {
        matches!(e, LiveEvent::Disconnected { provider, retry_in: Some(_), .. } if *provider == away.authority().to_string())
    })
    .await;
    told.until("connecting there again", |e| {
        matches!(e, LiveEvent::CaughtUp { provider, .. } if *provider == away.authority().to_string())
    })
    .await;
    let account = social
        .store()
        .social_account(&h.account())
        .unwrap()
        .unwrap();
    assert_eq!(account.state, SocialAccountState::Active);
}

#[tokio::test]
async fn presence_is_watched_where_the_user_is_hosted() {
    let h = home().await;
    let (bob, _, _) = h.bob_with_group();
    let (social, mut told) = live(&h, 1).await;
    assert_eq!(social.presence(&h.account(), &bob.to_string()), None);

    social
        .watch_presence(&h.account(), &[&bob.to_string()])
        .expect("watched");
    let event = told
        .until("Bob's presence", |e| {
            matches!(e, LiveEvent::Presence { .. })
        })
        .await;
    let LiveEvent::Presence { user, presence, .. } = event else {
        unreachable!()
    };
    assert_eq!(user, bob.to_string());
    assert_eq!(presence.availability, Availability::Offline);
    let known = social
        .presence(&h.account(), &bob.to_string())
        .expect("known");
    assert_eq!(known.availability, Availability::Offline);

    // Offline, nothing is known any more.
    social.go_offline(&h.account()).await.expect("offline");
    assert_eq!(social.presence(&h.account(), &bob.to_string()), None);
}

#[tokio::test]
async fn a_place_that_cannot_be_backfilled_is_tried_again_without_ending_the_session() {
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
    h.add().await;
    let place = channel_uri(&away, &group, &channel);
    // No cursor, and its pages refused: it cannot be backfilled for now.
    h.social
        .store()
        .change_social_place(&h.account(), &place, |p| p.cursors.live = None)
        .unwrap();
    away.faults().refuse_signatures_ending("/messages");
    let social = h.social.clone().with_live_options(super::options());
    let mut told = super::Told::new(&social);
    social.go_live(&h.account()).await.expect("live");
    // Both sessions catch up (home's own sync could not page it either),
    // and the place is left for the retry.
    told.caught_up(2).await;
    let away_caught_up = told.seen.iter().find(|e| {
        matches!(e, LiveEvent::CaughtUp { provider, .. } if *provider == away.authority().to_string())
    });
    assert!(
        matches!(away_caught_up, Some(LiveEvent::CaughtUp { backfilled, .. }) if backfilled.is_empty()),
        "{:#?}",
        told.seen
    );
    let cursor = |h: &Home| {
        h.social
            .store()
            .social_place(&h.account(), &place)
            .unwrap()
            .unwrap()
            .cursors
            .live
    };
    assert_eq!(cursor(&h), None);

    away.faults().refuse_signatures(false);
    let later = away.post_as(&dan, &group, &channel, "later").id.to_string();
    told.until("the message, once it is read", |e| is_new(e, &later))
        .await;
    assert_eq!(h.messages(&place).len(), 2);
    // The cursor is written once the backfill has kept its pages.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while cursor(&h).is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("it resumes from there");
    assert!(
        !told
            .seen
            .iter()
            .any(|e| matches!(e, LiveEvent::Disconnected { .. })),
        "the session stayed up: {:#?}",
        told.seen
    );
}
