//! Sockets failing: reconnecting and resuming, truncated replays read
//! over REST, closes that retry and closes that end, a clock that is off.

use postrinch_social::LiveEvent;
use postrinch_store::SocialAccountState;

use super::{Told, live, post_each};
use crate::common::{Home, PASSWORD, channel_uri, home, requests_to};

/// Drop the sockets and keep them from signing in again, until the
/// session has failed to reconnect at least once.
async fn go_offline(h: &Home, told: &mut Told) {
    h.provider.faults().refuse_ws_auth(true);
    h.provider.faults().drop_sockets();
    for _ in 0..2 {
        told.until("the session ending", |e| {
            matches!(
                e,
                LiveEvent::Disconnected {
                    retry_in: Some(_),
                    ..
                }
            )
        })
        .await;
    }
}

/// Let the sessions sign in again, and wait for the home one to catch up.
async fn come_back(h: &Home, told: &mut Told) -> Vec<String> {
    h.provider.faults().refuse_ws_auth(false);
    match told
        .until("catching up again", |e| {
            matches!(e, LiveEvent::CaughtUp { .. })
        })
        .await
    {
        LiveEvent::CaughtUp { backfilled, .. } => backfilled,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_reconnect_resumes_from_the_cursor_without_reading_pages() {
    let h = home().await;
    let (bob, group, general) = h.bob_with_group();
    h.provider.post_as(&bob, &group, &general, "before");
    let (_social, mut told) = live(&h, 1).await;
    let pages = format!("/api/groups/{group}/channels/{general}/messages");
    let read_before = requests_to(&h.provider, &pages).len();

    go_offline(&h, &mut told).await;
    let missed = post_each(&h.provider, &bob, &group, &general, &["one", "two"]).await;
    let backfilled = come_back(&h, &mut told).await;
    assert!(backfilled.is_empty(), "the resume replayed it all");
    for id in &missed {
        told.until("a missed message", |e| super::is_new(e, id))
            .await;
    }
    let place = channel_uri(&h.provider, &group, &general);
    let unread: Vec<_> = h.messages(&place).iter().map(|m| m.unread).collect();
    // Nothing is read at home yet (no marker there): all unread.
    assert_eq!(unread, [true, true, true]);
    assert_eq!(
        requests_to(&h.provider, &pages).len(),
        read_before,
        "no page was read again"
    );
}

#[tokio::test]
async fn a_truncated_replay_is_read_again_over_rest() {
    let h = home().await;
    let (bob, group, general) = h.bob_with_group();
    h.provider.post_as(&bob, &group, &general, "before");
    let (_social, mut told) = live(&h, 1).await;

    go_offline(&h, &mut told).await;
    h.provider.faults().set_replay_limit(Some(0));
    post_each(
        &h.provider,
        &bob,
        &group,
        &general,
        &["one", "two", "three"],
    )
    .await;
    let place = channel_uri(&h.provider, &group, &general);
    let backfilled = come_back(&h, &mut told).await;
    assert_eq!(backfilled, std::slice::from_ref(&place));
    let kept = h.messages(&place);
    let texts: Vec<_> = kept
        .iter()
        .map(|m| m.message["content"]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(texts, ["before", "one", "two", "three"]);
    let unread: Vec<_> = kept.iter().map(|m| m.unread).collect();
    assert_eq!(unread, [true, true, true, true]);

    // Live again from where the backfill ended.
    h.provider.faults().set_replay_limit(None);
    let next = h
        .provider
        .post_as(&bob, &group, &general, "four")
        .id
        .to_string();
    told.until("the next message", |e| super::is_new(e, &next))
        .await;
    assert_eq!(h.messages(&place).len(), 5);
}

#[tokio::test]
async fn close_4001_retries_and_4003_needs_sign_in() {
    let h = home().await;
    let (social, mut told) = live(&h, 1).await;
    social.keep_connected().await.expect("kept connected");

    h.provider.faults().close_sockets_auth_failed();
    told.until("the 4001", |e| {
        matches!(
            e,
            LiveEvent::Disconnected {
                retry_in: Some(_),
                ..
            }
        )
    })
    .await;
    told.caught_up(1).await;
    let account = social
        .store()
        .social_account(&h.account())
        .unwrap()
        .unwrap();
    assert_eq!(account.state, SocialAccountState::Active);

    let key = account.key_id.unwrap();
    assert!(h.provider.faults().revoke_key(&key));
    told.until("the 4003", |e| {
        matches!(e, LiveEvent::Disconnected { retry_in: None, .. })
    })
    .await;
    let event = told
        .until("the account's state", |e| {
            matches!(e, LiveEvent::AccountState { .. })
        })
        .await;
    assert_eq!(
        event,
        LiveEvent::AccountState {
            account: h.account(),
            state: SocialAccountState::NeedsSignIn,
        }
    );
    let account = social
        .store()
        .social_account(&h.account())
        .unwrap()
        .unwrap();
    assert_eq!(account.state, SocialAccountState::NeedsSignIn);
    assert!(
        social.sessions(&h.account()).is_empty(),
        "not live any more"
    );

    // Signing in again goes live again, as the engine is kept connected.
    social
        .sign_in_again(&h.account(), PASSWORD)
        .await
        .expect("signed in");
    told.caught_up(1).await;
    assert_eq!(social.sessions(&h.account()).len(), 1);
}

#[tokio::test]
async fn a_clock_far_off_still_connects_and_syncs() {
    let h = home().await;
    let (bob, group, general) = h.bob_with_group();
    let (social, mut told) = live(&h, 1).await;

    h.provider.faults().set_clock_offset(15 * 60);
    h.provider.faults().drop_sockets();
    told.caught_up(1).await;
    let posted = h
        .provider
        .post_as(&bob, &group, &general, "on time")
        .id
        .to_string();
    told.until("the message", |e| super::is_new(e, &posted))
        .await;
    let account = social
        .store()
        .social_account(&h.account())
        .unwrap()
        .unwrap();
    assert_eq!(account.state, SocialAccountState::Active);
}
