//! How the outbox meets sync: taking a change back, and a post settled by
//! the message its provider made of it.

use super::*;

fn post() -> SocialChange {
    SocialChange::Post(SocialPost {
        kind: ContentKind::Chat,
        reply_to: None,
        content: SocialContent {
            body: "hello".into(),
            ..SocialContent::default()
        },
    })
}

#[test]
fn a_change_is_taken_back_only_before_it_is_tried() {
    let (_dir, store) = ada_with_general();
    store
        .queue_social_change(ADA, "c1", Some(GENERAL), &post(), 1)
        .unwrap();
    store
        .queue_social_change(ADA, "c2", Some(GENERAL), &post(), 2)
        .unwrap();
    store
        .queue_social_change(ADA, "c3", Some(GENERAL), &post(), 3)
        .unwrap();
    assert_eq!(
        store.take_back_social_change(ADA, "c1").unwrap(),
        TakeBack::Taken
    );
    assert_eq!(
        store.take_back_social_change(ADA, "c1").unwrap(),
        TakeBack::NotQueued
    );

    let tried = store.social_change_tried(ADA, "c2").unwrap().unwrap();
    assert!(tried.tried);
    assert_eq!(
        store.take_back_social_change(ADA, "c2").unwrap(),
        TakeBack::Tried
    );
    store.social_change_failed(ADA, "c3", "timed out").unwrap();
    assert_eq!(
        store.take_back_social_change(ADA, "c3").unwrap(),
        TakeBack::Tried
    );
    assert_eq!(store.social_outbox(ADA).unwrap().len(), 2);
    assert_eq!(store.social_change_tried(ADA, "c1").unwrap(), None);
}

#[test]
fn a_kept_message_settles_the_post_it_was_sent_as() {
    let (_dir, store) = ada_with_general();
    let elsewhere = "https://groups.example/api/groups/g1/channels/other";
    store
        .queue_social_change(ADA, "c1", Some(GENERAL), &post(), 1)
        .unwrap();
    store
        .queue_social_change(ADA, "c2", Some(elsewhere), &post(), 2)
        .unwrap();
    store
        .queue_social_change(ADA, "c3", Some(GENERAL), &read_change("m0"), 3)
        .unwrap();

    // A message naming a post of another place, or a change that is no
    // post, settles nothing.
    let mut other = message(ADA, GENERAL, "m1", 10);
    other.message["clientMessageId"] = json!("c2");
    store.put_social_message(&other, "").unwrap();
    let mut odd = message(ADA, GENERAL, "m2", 11);
    odd.message["clientMessageId"] = json!("c3");
    store.put_social_message(&odd, "").unwrap();
    assert_eq!(store.social_outbox(ADA).unwrap().len(), 3);

    // Nor does someone else's message that happens to use the same id.
    let mut theirs = message(ADA, GENERAL, "m3", 12);
    theirs.message["clientMessageId"] = json!("c1");
    store.put_social_message(&theirs, "").unwrap();
    assert_eq!(store.social_outbox(ADA).unwrap().len(), 3);

    let mut sent = message(ADA, GENERAL, "m4", 13);
    sent.author = "Ada@Groups.example".into();
    sent.message["clientMessageId"] = json!("c1");
    store.put_social_message(&sent, "").unwrap();
    let left: Vec<String> = store
        .social_outbox(ADA)
        .unwrap()
        .into_iter()
        .map(|e| e.client_id)
        .collect();
    assert_eq!(left, ["c2", "c3"]);
}

#[test]
fn a_refused_change_stays_queued_and_says_why() {
    let (_dir, store) = ada_with_general();
    store
        .queue_social_change(ADA, "c1", Some(GENERAL), &post(), 1)
        .unwrap();
    let refused = store
        .social_change_refused(ADA, "c1", "403 forbidden")
        .unwrap()
        .unwrap();
    assert!(refused.refused);
    assert_eq!(refused.attempts, 1);
    assert_eq!(refused.error.as_deref(), Some("403 forbidden"));
    assert_eq!(store.social_outbox(ADA).unwrap(), [refused]);
    assert_eq!(store.social_change_refused(ADA, "c0", "x").unwrap(), None);
}

#[test]
fn the_persons_own_message_is_never_kept_unread() {
    let (_dir, store) = ada_with_general();
    let mut own = message(ADA, GENERAL, "m1", 1);
    own.author = "Ada@Groups.example".into();
    store.put_social_message(&own, "").unwrap();
    store
        .put_social_message(&message(ADA, GENERAL, "m2", 2), "")
        .unwrap();
    let kept = store.social_message(ADA, GENERAL, "m1").unwrap().unwrap();
    assert!(!kept.unread);
    assert_eq!(store.social_unread_count(ADA, GENERAL).unwrap(), 1);
    // The person may still mark it unread.
    store
        .set_social_message_unread(ADA, GENERAL, "m1", true)
        .unwrap();
    store.put_social_message(&own, "edited").unwrap();
    assert!(
        store
            .social_message(ADA, GENERAL, "m1")
            .unwrap()
            .unwrap()
            .unread
    );
}
