//! The outbox (the plan's PR 10): queued changes sent in order, per
//! place, retried safely, and kept as their providers answer. Changes are
//! queued in the store as the commands queue them.

mod changes;
mod dms;
mod order;
mod posts;

use postrinch_social::{LiveEvent, Sent};
use postrinch_store::{
    ContentKind, SocialChange, SocialContent, SocialMessage, SocialOutboxEntry, SocialPost,
};

use crate::common::Home;
use crate::live::live;

/// Queue `change` to `place` as Ada, under `client_id`.
pub fn queue(h: &Home, client_id: &str, place: &str, change: &SocialChange) {
    h.social
        .store()
        .queue_social_change(&h.account(), client_id, Some(place), change, 1)
        .expect("queued");
}

/// A chat line saying `body`, answering `reply_to` when given.
pub fn chat(body: &str, reply_to: Option<&str>) -> SocialChange {
    SocialChange::Post(SocialPost {
        kind: ContentKind::Chat,
        reply_to: reply_to.map(str::to_owned),
        content: text(body),
    })
}

/// Content saying `body`.
pub fn text(body: &str) -> SocialContent {
    SocialContent {
        body: body.into(),
        ..SocialContent::default()
    }
}

/// Ada's outbox, as queued.
pub fn outbox(h: &Home) -> Vec<SocialOutboxEntry> {
    h.social.store().social_outbox(&h.account()).unwrap()
}

/// One pass over Ada's outbox.
pub async fn send(h: &Home) -> Sent {
    h.social.send_outbox(&h.account()).await.expect("a pass")
}

/// Wait until Ada's outbox is empty, failing after a while.
pub async fn until_sent(h: &Home) {
    let waiting = async {
        while !outbox(h).is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    };
    if tokio::time::timeout(std::time::Duration::from_secs(15), waiting)
        .await
        .is_err()
    {
        panic!("still queued: {:#?}", outbox(h));
    }
}

/// The kept message sent as `client_id` in `place`.
pub fn kept_sent(h: &Home, place: &str, client_id: &str) -> SocialMessage {
    h.messages(place)
        .into_iter()
        .find(|m| m.message["clientMessageId"] == client_id)
        .unwrap_or_else(|| panic!("{client_id} is kept in {place}"))
}

#[tokio::test]
async fn a_live_account_sends_what_is_queued_when_told() {
    let h = crate::common::home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = crate::common::channel_uri(&h.provider, &group, &channel);
    let (social, mut told) = live(&h, 1).await;

    queue(&h, "c1", &place, &chat("hello", None));
    social.outbox_changed(&h.account());
    told.until(
        "the post sent",
        |e| matches!(e, LiveEvent::Sent { client_id, .. } if client_id == "c1"),
    )
    .await;
    assert!(outbox(&h).is_empty());
    let at_provider = h.provider.messages(&group, &channel);
    assert_eq!(at_provider.len(), 1);
    assert_eq!(at_provider[0].client_message_id.as_deref(), Some("c1"));
    social.go_offline(&h.account()).await.unwrap();
}

#[tokio::test]
async fn a_live_account_settles_a_post_whose_answer_was_dropped() {
    let h = crate::common::home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = crate::common::channel_uri(&h.provider, &group, &channel);
    let (social, mut told) = live(&h, 1).await;

    // The provider takes the post, but its answer never arrives.
    h.provider.faults().drop_answers("POST", "/messages", 1);
    queue(&h, "c1", &place, &chat("hello", None));
    social.outbox_changed(&h.account());
    // Settled either by its copy arriving on the socket (before or after
    // the lost answer is noticed), or by trying again by itself, with
    // backoff, matched to the copy the provider already made. Either way
    // it is sent, and not made twice.
    told.until(
        "the post sent",
        |e| matches!(e, LiveEvent::Sent { client_id, .. } if client_id == "c1"),
    )
    .await;
    until_sent(&h).await;
    assert_eq!(h.provider.messages(&group, &channel).len(), 1);
    assert_eq!(
        kept_sent(&h, &place, "c1").message["content"]["text"],
        "hello"
    );
    assert!(outbox(&h).is_empty());
    social.go_offline(&h.account()).await.unwrap();
}

#[tokio::test]
async fn a_live_account_tries_again_with_backoff_while_its_provider_fails() {
    let h = crate::common::home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = crate::common::channel_uri(&h.provider, &group, &channel);
    let (social, mut told) = live(&h, 1).await;

    // More 503s than the transport retries by itself: the pass fails.
    h.provider.faults().fail_next(4, None);
    queue(&h, "c1", &place, &chat("hello", None));
    social.outbox_changed(&h.account());
    told.until(
        "the post failing",
        |e| matches!(e, LiveEvent::NotSent { client_id, refused: false, .. } if client_id == "c1"),
    )
    .await;
    told.until(
        "the post sent",
        |e| matches!(e, LiveEvent::Sent { client_id, .. } if client_id == "c1"),
    )
    .await;
    assert_eq!(h.provider.messages(&group, &channel).len(), 1);
    assert!(outbox(&h).is_empty());
    social.go_offline(&h.account()).await.unwrap();
}

#[tokio::test]
async fn an_account_that_goes_live_sends_what_waited() {
    let h = crate::common::home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = crate::common::channel_uri(&h.provider, &group, &channel);
    h.add().await;
    queue(&h, "c1", &place, &chat("queued while away", None));

    let social = h.social.clone().with_live_options(crate::live::options());
    let mut told = crate::live::Told::new(&social);
    social.go_live(&h.account()).await.unwrap();
    told.until(
        "the post sent",
        |e| matches!(e, LiveEvent::Sent { client_id, .. } if client_id == "c1"),
    )
    .await;
    assert_eq!(h.provider.messages(&group, &channel).len(), 1);
    social.go_offline(&h.account()).await.unwrap();
}
