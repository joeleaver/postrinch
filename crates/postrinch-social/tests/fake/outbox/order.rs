//! Changes go in the order they were made, per place: one that fails
//! holds back those after it in its place, and no other place.

use crate::common::{channel_uri, home};

use super::{chat, outbox, queue, send, text};
use postrinch_store::SocialChange;

#[tokio::test]
async fn a_failing_change_holds_back_its_place_alone() {
    let h = home().await;
    let (_bob, group, general) = h.bob_with_group();
    let random = h.provider.create_channel(&group, "random");
    h.add().await;
    let (a, b) = (
        channel_uri(&h.provider, &group, &general),
        channel_uri(&h.provider, &group, &random),
    );

    // The first post to general is taken, but its answer is lost.
    let general_path = format!("/channels/{general}/messages");
    h.provider.faults().drop_answers("POST", &general_path, 1);
    queue(&h, "a1", &a, &chat("one", None));
    queue(&h, "a2", &a, &chat("two", None));
    queue(&h, "b1", &b, &chat("elsewhere", None));
    queue(&h, "a3", &a, &chat("three", None));

    let first = send(&h).await;
    assert_eq!((first.sent, first.failed), (1, 1));
    let left: Vec<String> = outbox(&h).into_iter().map(|e| e.client_id).collect();
    assert_eq!(left, ["a1", "a2", "a3"]);
    // Only the failing one was tried; those after it waited.
    assert_eq!(h.provider.messages(&group, &general).len(), 1);
    assert_eq!(h.provider.messages(&group, &random).len(), 1);
    assert!(outbox(&h).iter().skip(1).all(|e| !e.tried));

    let second = send(&h).await;
    assert_eq!((second.sent, second.failed), (3, 0));
    let sent: Vec<Option<String>> = h
        .provider
        .messages(&group, &general)
        .into_iter()
        .map(|m| m.client_message_id)
        .collect();
    assert_eq!(
        sent,
        [Some("a1".into()), Some("a2".into()), Some("a3".into())]
    );
}

#[tokio::test]
async fn a_refused_change_is_kept_and_holds_back_nothing() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    let bobs = h.provider.post_as(&bob, &group, &channel, "Bob's");
    h.add().await;

    // Ada may not edit Bob's message.
    let edit = SocialChange::Edit {
        message: bobs.id.to_string(),
        content: text("not mine to change"),
    };
    queue(&h, "e1", &place, &edit);
    queue(&h, "c1", &place, &chat("after it", None));
    let sent = send(&h).await;
    assert_eq!((sent.sent, sent.refused), (1, 1));
    let kept = outbox(&h);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].client_id, "e1");
    assert!(kept[0].refused);
    assert!(kept[0].error.as_deref().is_some_and(|e| e.contains("403")));

    // It is not tried again.
    let again = send(&h).await;
    assert_eq!(again, postrinch_social::Sent::default());
    assert_eq!(outbox(&h)[0].attempts, 1);
}
