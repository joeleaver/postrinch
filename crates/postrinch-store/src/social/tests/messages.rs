use serde_json::json;

use super::*;
use crate::social::messages::CONTENT;

fn reply(id: &str, parent: &str, created: i64) -> SocialMessage {
    SocialMessage {
        parent: Some(parent.into()),
        ..message(ADA, GENERAL, id, created)
    }
}

fn ids(messages: &[SocialMessage]) -> Vec<&str> {
    messages.iter().map(|m| m.id.as_str()).collect()
}

#[test]
fn a_message_reads_back_as_it_was_kept() {
    let (_dir, store) = ada_with_general();
    let mut kept = reply("m2", "m1", 5);
    kept.account = "Ada@Groups.Example".into();
    kept.reactions = vec![ReactionSum {
        key: "👍".into(),
        count: 2,
        mine: true,
    }];
    kept.mentions_me = true;
    assert!(store.put_social_message(&kept, "hello ada").unwrap());
    let read = store.social_message(ADA, GENERAL, "m2").unwrap().unwrap();
    assert_eq!(
        read,
        SocialMessage {
            account: ADA.into(),
            ..kept.clone()
        }
    );
    // A replayed event changes nothing; an edit does.
    assert!(!store.put_social_message(&kept, "hello ada").unwrap());
    assert!(
        store
            .put_social_message(&kept, "hello ada, edited")
            .unwrap()
    );
    let mut edited = kept.clone();
    edited.message = json!({ "id": "m2", "content": { "text": "edited" } });
    assert!(store.put_social_message(&edited, "edited").unwrap());
    let read = store.social_message(ADA, GENERAL, "m2").unwrap().unwrap();
    assert_eq!(read.message, edited.message);
    assert_eq!(store.db.count_type(CONTENT).unwrap(), 1);
}

#[test]
fn a_place_pages_back_through_its_messages_newest_first() {
    let (_dir, store) = ada_with_general();
    for (id, created) in [("m1", 1), ("m2", 2), ("m3", 2), ("m4", 4)] {
        store
            .put_social_message(&message(ADA, GENERAL, id, created), id)
            .unwrap();
    }
    store
        .put_social_message(&reply("r1", "m1", 9), "r1")
        .unwrap();
    // Another account's copy of the place is another place.
    store
        .put_social_message(&message(BOB, GENERAL, "m9", 9), "m9")
        .unwrap();

    let first = store.social_messages(ADA, GENERAL, None, 2).unwrap();
    assert_eq!(ids(&first), ["m4", "m3"]);
    let last = &first[1];
    let next = store
        .social_messages(ADA, GENERAL, Some((last.created, &last.id)), 2)
        .unwrap();
    assert_eq!(ids(&next), ["m2", "m1"], "a tie in time is not skipped");
    let last = &next[1];
    let end = store
        .social_messages(ADA, GENERAL, Some((last.created, &last.id)), 2)
        .unwrap();
    assert!(end.is_empty());
}

#[test]
fn replies_list_oldest_first_and_are_counted() {
    let (_dir, store) = ada_with_general();
    store
        .put_social_message(&message(ADA, GENERAL, "m1", 1), "m1")
        .unwrap();
    for (id, created) in [("r2", 3), ("r1", 2)] {
        store
            .put_social_message(&reply(id, "m1", created), id)
            .unwrap();
    }
    let replies = store.social_replies(ADA, GENERAL, "m1").unwrap();
    assert_eq!(ids(&replies), ["r1", "r2"]);
    assert_eq!(replies[0].parent.as_deref(), Some("m1"));
    assert_eq!(store.social_reply_count(ADA, GENERAL, "m1").unwrap(), 2);
    assert_eq!(store.social_reply_count(ADA, GENERAL, "r1").unwrap(), 0);
    assert_eq!(store.social_reply_count(BOB, GENERAL, "m1").unwrap(), 0);
}

#[test]
fn messages_are_read_up_to_a_time_or_one_at_a_time() {
    let (_dir, store) = ada_with_general();
    for (id, created) in [("m1", 1), ("m2", 2), ("m3", 3)] {
        store
            .put_social_message(&message(ADA, GENERAL, id, created), id)
            .unwrap();
    }
    assert_eq!(store.social_unread_count(ADA, GENERAL).unwrap(), 3);
    assert_eq!(store.set_social_messages_read(ADA, GENERAL, 2).unwrap(), 2);
    assert_eq!(store.set_social_messages_read(ADA, GENERAL, 2).unwrap(), 0);
    assert_eq!(store.social_unread_count(ADA, GENERAL).unwrap(), 1);
    assert!(
        store
            .set_social_message_unread(ADA, GENERAL, "m1", true)
            .unwrap()
    );
    assert!(
        !store
            .set_social_message_unread(ADA, GENERAL, "m0", true)
            .unwrap()
    );
    let m1 = store.social_message(ADA, GENERAL, "m1").unwrap().unwrap();
    assert!(m1.unread);
    assert_eq!(store.social_unread_count(ADA, GENERAL).unwrap(), 2);
}

#[test]
fn mentions_of_every_account_list_newest_first() {
    let (_dir, store) = ada_with_general();
    let mention = |account: &str, id: &str, created| SocialMessage {
        mentions_me: true,
        ..message(account, GENERAL, id, created)
    };
    store
        .put_social_message(&mention(ADA, "m1", 1), "m1")
        .unwrap();
    store
        .put_social_message(&mention(BOB, "m2", 3), "m2")
        .unwrap();
    store
        .put_social_message(&mention(ADA, "m3", 2), "m3")
        .unwrap();
    store
        .put_social_message(&message(ADA, GENERAL, "m4", 4), "m4")
        .unwrap();
    assert_eq!(ids(&store.social_mentions(10).unwrap()), ["m2", "m3", "m1"]);
    assert_eq!(ids(&store.social_mentions(1).unwrap()), ["m2"]);
}

#[test]
fn messages_are_found_by_their_words() {
    let (_dir, store) = ada_with_general();
    store
        .put_social_message(&message(ADA, GENERAL, "m1", 1), "the rocket launched")
        .unwrap();
    store
        .put_social_message(&message(ADA, GENERAL, "m2", 2), "lunch at noon")
        .unwrap();
    assert_eq!(
        ids(&store.search_social_messages("rocket", 10).unwrap()),
        ["m1"]
    );
    // Edited words are found, the old ones no longer.
    store
        .put_social_message(&message(ADA, GENERAL, "m1", 1), "the boat sailed")
        .unwrap();
    assert!(
        store
            .search_social_messages("rocket", 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ids(&store.search_social_messages("boat", 10).unwrap()),
        ["m1"]
    );
}

#[test]
fn a_deleted_message_is_kept_marked_and_can_be_forgotten() {
    let (_dir, store) = ada_with_general();
    let mut gone = message(ADA, GENERAL, "m1", 1);
    store.put_social_message(&gone, "m1").unwrap();
    gone.deleted = true;
    store.put_social_message(&gone, "").unwrap();
    assert!(
        store
            .social_message(ADA, GENERAL, "m1")
            .unwrap()
            .unwrap()
            .deleted
    );
    store.remove_social_message(ADA, GENERAL, "m1").unwrap();
    assert_eq!(store.social_message(ADA, GENERAL, "m1").unwrap(), None);
    assert_eq!(store.db.count_type(CONTENT).unwrap(), 0);
}

#[test]
fn an_interrupted_put_is_not_read_and_its_repeat_finishes_it() {
    let (_dir, store) = ada_with_general();
    let kept = message(ADA, GENERAL, "m1", 1);
    store.put_social_message(&kept, "m1").unwrap();
    // As if the put had stopped between the message and its content.
    let key = format!("{ADA}\u{1f}{GENERAL}\u{1f}m1");
    let content = store.find(CONTENT, "key", &key).unwrap().unwrap();
    store.db_delete(CONTENT, content.id()).unwrap();
    assert_eq!(store.social_message(ADA, GENERAL, "m1").unwrap(), None);
    assert!(
        store
            .social_messages(ADA, GENERAL, None, 10)
            .unwrap()
            .is_empty()
    );
    assert!(store.put_social_message(&kept, "m1").unwrap());
    assert_eq!(
        store.social_message(ADA, GENERAL, "m1").unwrap(),
        Some(kept)
    );
}
