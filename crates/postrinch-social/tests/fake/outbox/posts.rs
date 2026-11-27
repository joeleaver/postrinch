//! Posts: sent, kept, and matched to their provider's copy by their
//! client id, however many times they are sent.

use crate::common::{channel_uri, home, requests_to};

use super::{chat, kept_sent, outbox, queue, send};

#[tokio::test]
async fn a_post_is_sent_and_its_copy_settles_it() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    let parent = h.provider.post_as(&bob, &group, &channel, "a question");
    h.add().await;

    queue(
        &h,
        "c1",
        &place,
        &chat("an answer", Some(parent.id.as_str())),
    );
    let sent = send(&h).await;
    assert_eq!((sent.sent, sent.failed, sent.refused), (1, 0, 0));
    assert!(outbox(&h).is_empty());

    let at_provider = h.provider.messages(&group, &channel);
    let reply = at_provider
        .iter()
        .find(|m| m.client_message_id.as_deref() == Some("c1"))
        .expect("the provider has it");
    let reference = reply.reference.as_ref().expect("a reply");
    // A channel's reply names a parent the channel holds: no provider.
    assert_eq!(
        (reference.id.as_str(), &reference.provider),
        (parent.id.as_str(), &None)
    );
    assert_eq!(reply.content.as_ref().unwrap().mime, "text/markdown");

    let kept = kept_sent(&h, &place, "c1");
    assert_eq!(kept.id, reply.id.as_str());
    assert_eq!(kept.parent.as_deref(), Some(parent.id.as_str()));
    assert!(!kept.unread);
    // Nothing is left to send.
    assert_eq!(send(&h).await, postrinch_social::Sent::default());
}

#[tokio::test]
async fn a_post_whose_answer_was_lost_is_matched_by_its_client_id() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;

    h.provider.faults().drop_answers("POST", "/messages", 1);
    queue(&h, "c1", &place, &chat("hello", None));
    let first = send(&h).await;
    assert_eq!((first.sent, first.failed), (0, 1));
    let waiting = outbox(&h);
    assert_eq!(waiting.len(), 1);
    assert!(waiting[0].tried && waiting[0].error.is_some() && !waiting[0].refused);
    // The provider took it all the same.
    assert_eq!(h.provider.messages(&group, &channel).len(), 1);

    let second = send(&h).await;
    assert_eq!((second.sent, second.failed), (1, 0));
    assert_eq!(h.provider.messages(&group, &channel).len(), 1);
    assert_eq!(h.messages(&place).len(), 1);
    assert!(outbox(&h).is_empty());

    // Each attempt was signed afresh: the retry is a new request, and the
    // provider answered it with the first copy.
    let path = format!("/api/groups/{group}/channels/{channel}/messages");
    let posts: Vec<_> = requests_to(&h.provider, &path)
        .into_iter()
        .filter(|r| r.method == "POST")
        .collect();
    assert_eq!(posts.len(), 2);
    assert_ne!(posts[0].nonce, posts[1].nonce);
    assert_eq!(posts[1].status, 200);
}

#[tokio::test]
async fn the_outbox_survives_a_restart() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;

    // Sent once, its answer lost, then the app quits.
    h.provider.faults().drop_answers("POST", "/messages", 1);
    queue(&h, "c1", &place, &chat("before the restart", None));
    assert_eq!(send(&h).await.failed, 1);

    // A new engine over the same store and keychain sends it, and it is
    // still made only once.
    let after = h.social_with_keys(h.keys.clone());
    let sent = after.send_outbox(&h.account()).await.unwrap();
    assert_eq!(sent.sent, 1);
    assert!(outbox(&h).is_empty());
    let at_provider = h.provider.messages(&group, &channel);
    assert_eq!(at_provider.len(), 1);
    assert_eq!(kept_sent(&h, &place, "c1").id, at_provider[0].id.as_str());
}

#[tokio::test]
async fn a_post_with_a_file_not_yet_uploaded_waits_untried() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;

    let mut post = chat("with a picture", None);
    if let postrinch_store::SocialChange::Post(p) = &mut post {
        p.content
            .attachments
            .push(postrinch_store::DraftMedia::File {
                path: "/tmp/picture.png".into(),
                mime: Some("image/png".into()),
            });
    }
    queue(&h, "c1", &place, &post);
    queue(&h, "c2", &place, &chat("after it", None));
    let sent = send(&h).await;
    assert_eq!((sent.sent, sent.waiting), (0, 1));
    // Untried, so it may still be taken back; what follows it waits.
    let waiting = outbox(&h);
    assert_eq!(waiting.len(), 2);
    assert!(!waiting[0].tried && !waiting[1].tried);
    assert!(h.provider.messages(&group, &channel).is_empty());
}
