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
    assert_eq!(store.put_social_message(&other, "").unwrap().settled, None);
    let mut odd = message(ADA, GENERAL, "m2", 11);
    odd.message["clientMessageId"] = json!("c3");
    assert_eq!(store.put_social_message(&odd, "").unwrap().settled, None);
    assert_eq!(store.social_outbox(ADA).unwrap().len(), 3);

    // Nor does someone else's message that happens to use the same id.
    let mut theirs = message(ADA, GENERAL, "m3", 12);
    theirs.message["clientMessageId"] = json!("c1");
    assert_eq!(store.put_social_message(&theirs, "").unwrap().settled, None);
    assert_eq!(store.social_outbox(ADA).unwrap().len(), 3);

    let mut sent = message(ADA, GENERAL, "m4", 13);
    sent.author = "Ada@Groups.example".into();
    sent.message["clientMessageId"] = json!("c1");
    let put = store.put_social_message(&sent, "").unwrap();
    assert_eq!(put.settled.as_deref(), Some("c1"));
    // Its copy kept again settles nothing more, changed or not.
    let again = store.put_social_message(&sent, "").unwrap();
    assert_eq!(again, MessagePut::default());
    sent.message["content"] = json!({ "text": "edited" });
    assert_eq!(store.put_social_message(&sent, "").unwrap().settled, None);
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

#[test]
fn an_uploaded_file_is_kept_in_the_change_and_it_can_still_be_taken_back() {
    let (_dir, store) = ada_with_general();
    let picture = DraftMedia::File {
        path: "/home/ada/cat.png".into(),
        mime: None,
    };
    let change = SocialChange::Post(SocialPost {
        kind: ContentKind::Article,
        reply_to: None,
        content: SocialContent {
            body: "a cat".into(),
            cover: Some(picture.clone()),
            attachments: vec![picture.clone()],
            ..SocialContent::default()
        },
    });
    store
        .queue_social_change(ADA, "c1", Some(GENERAL), &change, 1)
        .unwrap();
    assert_eq!(picture.file_name().as_deref(), Some("cat.png"));
    assert_eq!(picture.mime(), "image/png");

    // A failed upload says why, but is no attempt: nothing was sent.
    let failed = store
        .social_change_not_uploaded(ADA, "c1", "the connection broke")
        .unwrap()
        .unwrap();
    assert_eq!(failed.error.as_deref(), Some("the connection broke"));
    assert_eq!((failed.tried, failed.attempts), (false, 0));

    let attachment = serde_json::json!({
        "id": "att_1", "mime": "image/png", "url": "https://groups.example/m/1", "size": 3
    });
    let entry = store
        .social_change_uploaded(ADA, "c1", &picture, attachment.clone())
        .unwrap()
        .unwrap();
    let SocialChange::Post(post) = &entry.change else {
        panic!("a post");
    };
    let uploaded = DraftMedia::Uploaded { attachment };
    assert_eq!(post.content.cover.as_ref(), Some(&uploaded));
    assert_eq!(post.content.attachments, [uploaded]);
    assert_eq!(store.social_outbox(ADA).unwrap(), [entry]);

    assert_eq!(
        store.take_back_social_change(ADA, "c1").unwrap(),
        TakeBack::Taken
    );
    assert_eq!(
        store
            .social_change_uploaded(ADA, "c1", &picture, serde_json::json!({}))
            .unwrap(),
        None
    );
}
