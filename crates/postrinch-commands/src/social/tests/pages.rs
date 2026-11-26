//! Pages of a place and of a view, and threads.

use postrinch_store::{ContentKind, PlaceRef};

use super::*;
use crate::{HOME, MENTIONS};

/// A reply from Bob to `parent`.
fn reply(world: &World, place: &str, id: &str, parent: &str, created: i64) {
    let mut message = message(ADA, place, id, created, BOB, "a reply");
    message.parent = Some(parent.into());
    world.put(&message);
}

#[test]
fn a_place_pages_newest_first_without_replies() {
    let world = world();
    for (id, at) in [("a", 1), ("b", 2), ("c", 3), ("d", 4), ("e", 5)] {
        world.bob(GENERAL, id, at, id);
    }
    reply(&world, GENERAL, "r1", "b", 6);
    reply(&world, GENERAL, "r2", "b", 7);

    let first = world
        .commands
        .social_place_page(ADA, GENERAL, None, 2)
        .unwrap();
    assert_eq!(ids(&first.items), ["e", "d"]);
    let second = world
        .commands
        .social_place_page(ADA, GENERAL, first.older.as_ref(), 2)
        .unwrap();
    assert_eq!(ids(&second.items), ["c", "b"]);
    assert_eq!(second.items[1].replies, 2);
    let last = world
        .commands
        .social_place_page(ADA, GENERAL, second.older.as_ref(), 2)
        .unwrap();
    assert_eq!(ids(&last.items), ["a"]);
    assert_eq!(last.older, None);
    assert!(
        refused(world.commands.social_place_page(ADA, GENERAL, None, 0)).contains("at least one")
    );
    assert!(
        refused(world.commands.social_place_page(ADA, "nowhere", None, 5))
            .contains("knows no place")
    );
}

#[test]
fn messages_made_at_one_time_page_without_loss() {
    let world = world();
    for id in ["a", "b", "c", "d"] {
        world.bob(GENERAL, id, 10, id);
    }
    let mut seen = Vec::new();
    let mut before = None;
    loop {
        let page = world
            .commands
            .social_place_page(ADA, GENERAL, before.as_ref(), 1)
            .unwrap();
        seen.extend(ids(&page.items).into_iter().map(str::to_owned));
        match page.older {
            Some(cursor) => before = Some(cursor),
            None => break,
        }
    }
    assert_eq!(seen, ["d", "c", "b", "a"]);
}

#[test]
fn a_view_merges_its_places_and_filters_them() {
    let world = world();
    for (place, id, at, kind) in [
        (GENERAL, "g1", 1, "memo"),
        (GENERAL, "g2", 3, "message"),
        (CARL_CHANNEL, "c1", 2, "article"),
        (GENERAL, "g3", 4, "article"),
    ] {
        let account = if place == CARL_CHANNEL { CARL } else { ADA };
        let mut message = message(account, place, id, at, BOB, id);
        message.kind = kind.into();
        world.put(&message);
    }
    // Home reads memos and articles only, from every followed channel.
    let page = world
        .commands
        .social_view_page(HOME, None, None, 10)
        .unwrap();
    assert_eq!(ids(&page.items), ["g3", "c1", "g1"]);
    let first = world
        .commands
        .social_view_page(HOME, None, None, 2)
        .unwrap();
    assert_eq!(ids(&first.items), ["g3", "c1"]);
    let rest = world
        .commands
        .social_view_page(HOME, None, first.older.as_ref(), 2)
        .unwrap();
    assert_eq!(ids(&rest.items), ["g1"]);

    // One tab: one place.
    let tab = PlaceRef {
        account: CARL.into(),
        id: CARL_CHANNEL.into(),
    };
    let page = world
        .commands
        .social_view_page(HOME, Some(&tab), None, 10)
        .unwrap();
    assert_eq!(ids(&page.items), ["c1"]);
    let elsewhere = PlaceRef {
        account: ADA.into(),
        id: DM.into(),
    };
    assert!(
        refused(
            world
                .commands
                .social_view_page(HOME, Some(&elsewhere), None, 10)
        )
        .contains("isn't one of")
    );
    assert!(
        refused(
            world
                .commands
                .social_view_page(crate::DRAFTS, None, None, 10)
        )
        .contains("not messages")
    );
}

#[test]
fn mentions_lists_replies_too() {
    let world = world();
    world.bob(GENERAL, "root", 1, "root");
    let mut answer = message(ADA, GENERAL, "answer", 2, BOB, "@ada look");
    answer.parent = Some("root".into());
    answer.mentions_me = true;
    world.put(&answer);
    let mut elsewhere = message(ADA, DM, "dm", 3, BOB, "@ada hi");
    elsewhere.mentions_me = true;
    world.put(&elsewhere);
    let page = world
        .commands
        .social_view_page(MENTIONS, None, None, 10)
        .unwrap();
    assert_eq!(ids(&page.items), ["dm", "answer"]);
    let first = world
        .commands
        .social_view_page(MENTIONS, None, None, 1)
        .unwrap();
    let rest = world
        .commands
        .social_view_page(MENTIONS, None, first.older.as_ref(), 1)
        .unwrap();
    assert_eq!(ids(&rest.items), ["answer"]);
}

#[test]
fn a_post_being_sent_shows_at_once_and_once() {
    let world = world();
    world.bob(GENERAL, "old", 1, "old");
    let sent = world
        .commands
        .send_social(&world.draft(GENERAL, None, ContentKind::Chat, "mine"))
        .unwrap();
    assert!(!sent.posted);
    assert_eq!(sent.pending.as_ref().unwrap().client_id, sent.message.id);
    let page = world
        .commands
        .social_place_page(ADA, GENERAL, None, 10)
        .unwrap();
    assert_eq!(ids(&page.items), [sent.message.id.as_str(), "old"]);
    assert_eq!(page.items[0].message.author, ADA);
    assert!(!page.items[0].message.unread);

    // Sync keeps it, before the engine takes the change off the outbox:
    // it shows once, as sent.
    let mut kept = message(ADA, GENERAL, "p1", sent.message.created, ADA, "mine");
    kept.message["clientMessageId"] = sent.message.id.clone().into();
    world.put(&kept);
    let page = world
        .commands
        .social_place_page(ADA, GENERAL, None, 10)
        .unwrap();
    assert_eq!(ids(&page.items), ["p1", "old"]);
    assert!(page.items[0].posted);
}

#[test]
fn a_thread_is_a_message_and_its_replies_those_being_sent_among_them() {
    let world = world();
    world.bob(GENERAL, "root", 1, "root");
    reply(&world, GENERAL, "r1", "root", 2);
    let queued = world
        .commands
        .send_social(&world.draft(GENERAL, Some("r1"), ContentKind::Chat, "me too"))
        .unwrap();
    // Answering a reply answers its thread (§7.2, one level deep).
    assert_eq!(queued.message.parent.as_deref(), Some("root"));

    let thread = world
        .commands
        .social_thread(ADA, GENERAL, "r1")
        .unwrap()
        .unwrap();
    assert_eq!(thread.root.message.id, "root");
    assert_eq!(thread.root.replies, 2);
    assert_eq!(ids(&thread.replies), ["r1", queued.message.id.as_str()]);
    assert_eq!(thread.place.name, "general");
    let page = world
        .commands
        .social_place_page(ADA, GENERAL, None, 10)
        .unwrap();
    assert_eq!(page.items[0].replies, 2);

    // Once sync keeps it, it is counted once.
    let mut kept = message(ADA, GENERAL, "r2", 3, ADA, "me too");
    kept.parent = Some("root".into());
    kept.message["clientMessageId"] = queued.message.id.clone().into();
    world.put(&kept);
    let thread = world
        .commands
        .social_thread(ADA, GENERAL, "root")
        .unwrap()
        .unwrap();
    assert_eq!(ids(&thread.replies), ["r1", "r2"]);
    let page = world
        .commands
        .social_place_page(ADA, GENERAL, None, 10)
        .unwrap();
    assert_eq!(page.items[0].replies, 2);

    assert!(
        world
            .commands
            .social_thread(ADA, GENERAL, "none")
            .unwrap()
            .is_none()
    );
    let alone = world
        .commands
        .send_social(&world.draft(GENERAL, None, ContentKind::Chat, "new"))
        .unwrap();
    let thread = world
        .commands
        .social_thread(ADA, GENERAL, &alone.message.id)
        .unwrap()
        .unwrap();
    assert!(!thread.root.posted);
    assert!(thread.replies.is_empty());
}
