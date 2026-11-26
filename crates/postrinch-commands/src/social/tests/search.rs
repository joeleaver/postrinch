//! Searching every place the caller sees.

use super::*;

#[test]
fn words_are_found_through_the_index_and_narrowed() {
    let world = world();
    world.bob(GENERAL, "g1", 1, "the quick brown fox");
    world.bob(NEWS, "n1", 2, "a quick note");
    let mut tagged = message(ADA, NEWS, "n2", 3, ADA, "quick, tagged");
    tagged.message["tags"] = json!(["#rust"]);
    world.put(&tagged);
    world.put(&message(
        CARL,
        CARL_CHANNEL,
        "c1",
        4,
        BOB,
        "quick from carl",
    ));

    let mut found = ids(&world.commands.search_social("quick", 10).unwrap())
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    found.sort();
    assert_eq!(found, ["c1", "g1", "n1", "n2"]);
    let found = world.commands.search_social("\"brown fox\"", 10).unwrap();
    assert_eq!(ids(&found), ["g1"]);
    let found = world.commands.search_social("quick from:ada", 10).unwrap();
    assert_eq!(ids(&found), ["n2"]);
    let found = world.commands.search_social("quick tag:Rust", 10).unwrap();
    assert_eq!(ids(&found), ["n2"]);
    assert_eq!(world.commands.search_social("quick", 2).unwrap().len(), 2);
}

#[test]
fn without_words_every_message_named_is_listed_newest_first() {
    let world = world();
    world.bob(GENERAL, "g1", 1, "one");
    world.bob(DM, "d1", 3, "two");
    world.put(&message(ADA, NEWS, "n1", 2, ADA, "mine"));
    let found = world.commands.search_social("from:bob", 10).unwrap();
    assert_eq!(ids(&found), ["d1", "g1"]);
    let found = world
        .commands
        .search_social("from:bob@far.example", 1)
        .unwrap();
    assert_eq!(ids(&found), ["d1"]);
    assert!(refused(world.commands.search_social("   ", 10)).contains("Search for"));
    assert!(refused(world.commands.search_social("from:bob", 0)).contains("at least one"));
}

#[test]
fn a_search_shows_messages_as_changed() {
    let world = world();
    world.put(&message(ADA, GENERAL, "m1", 1, ADA, "draft wording"));
    world
        .commands
        .edit_social_message(
            ADA,
            GENERAL,
            "m1",
            &postrinch_store::SocialContent {
                body: "final wording".into(),
                ..Default::default()
            },
        )
        .unwrap();
    // The index still has the old words; what shows is the edit, which
    // no longer has them.
    assert!(
        world
            .commands
            .search_social("draft", 10)
            .unwrap()
            .is_empty()
    );
    let found = world.commands.search_social("from:ada", 10).unwrap();
    assert_eq!(found[0].message.message["content"]["text"], "final wording");
}
