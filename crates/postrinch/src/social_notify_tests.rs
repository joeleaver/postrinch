use postrinch_commands::{MESSAGES, SocialArrived};
use postrinch_store::PlaceRef;

use super::*;
use crate::notices::Several;

fn arrived(id: &str, conversation: bool) -> SocialArrived {
    SocialArrived {
        place: PlaceRef {
            account: "sam@forumall.example".into(),
            id: if conversation {
                "dm_mira".into()
            } else {
                "https://forumall.example/c/general".into()
            },
        },
        place_name: if conversation {
            "mira@forumall.example".into()
        } else {
            "general".into()
        },
        group_name: (!conversation).then(|| "Rust Audio".into()),
        conversation,
        message: id.into(),
        author: "mira@forumall.example".into(),
        created: 1,
        title: None,
        snippet: "could you post the mixer benchmark?".into(),
        called: true,
        view: MESSAGES.into(),
    }
}

#[test]
fn a_notice_says_who_where_and_what_as_the_settings_ask() {
    let channel = arrived("m1", false);
    assert_eq!(
        one(&channel, Shows::Everything),
        (
            "mira@forumall.example in Rust Audio \u{203a} general".to_owned(),
            "could you post the mixer benchmark?".to_owned()
        )
    );
    assert_eq!(one(&channel, Shows::WhoAndSubject).1, "");
    assert_eq!(
        one(&channel, Shows::Nothing),
        (
            "New message in Rust Audio \u{203a} general".to_owned(),
            String::new()
        )
    );
    let dm = arrived("d1", true);
    assert_eq!(one(&dm, Shows::Everything).0, "mira@forumall.example");
    assert_eq!(one(&dm, Shows::Nothing).0, "New private message");
}

#[test]
fn an_articles_notice_leads_with_its_title() {
    let mut post = arrived("a1", false);
    post.title = Some("Building a ladder filter".into());
    assert_eq!(
        one(&post, Shows::Everything).1,
        "Building a ladder filter\ncould you post the mixer benchmark?"
    );
    assert_eq!(
        one(&post, Shows::WhoAndSubject).1,
        "Building a ladder filter"
    );
}

#[test]
fn several_messages_become_one_notice_as_the_settings_ask() {
    assert!(!Several::EachUpToThree.together(3));
    assert!(Several::EachUpToThree.together(4));
    assert!(!Several::OneForAll.together(1));
    assert!(Several::OneForAll.together(2));
    let found = [
        arrived("m1", false),
        arrived("m2", false),
        arrived("d1", true),
    ];
    assert_eq!(
        all(&found, Shows::Everything),
        (
            "3 new messages in Rust Audio \u{203a} general, private messages".to_owned(),
            "mira@forumall.example".to_owned()
        )
    );
    assert_eq!(all(&found, Shows::Nothing).1, "");
}

#[test]
fn each_message_is_told_once() {
    let first = fresh(vec![arrived("once-1", false), arrived("once-2", true)]);
    assert_eq!(first.len(), 2);
    let again = fresh(vec![arrived("once-1", false), arrived("once-3", false)]);
    let ids: Vec<&str> = again.iter().map(|a| a.message.as_str()).collect();
    assert_eq!(ids, ["once-3"]);
}
