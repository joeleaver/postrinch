//! Views: the built-in ones, the person's own, their places and tallies.

use postrinch_store::{
    ContentKind, GroupRef, PlaceRef, ReadAs, SocialCount, SocialFilters, SocialView,
};

use super::*;
use crate::{ALL_CHANNELS, BuiltInView, DRAFTS, HOME, MENTIONS, MESSAGES};

fn view(name: &str, places: &[(&str, &str)], groups: &[(&str, &str)]) -> SocialView {
    SocialView {
        id: String::new(),
        order: 0,
        name: name.into(),
        places: places
            .iter()
            .map(|(account, id)| PlaceRef {
                account: (*account).into(),
                id: (*id).into(),
            })
            .collect(),
        groups: groups
            .iter()
            .map(|(account, uri)| GroupRef {
                account: (*account).into(),
                uri: (*uri).into(),
            })
            .collect(),
        filters: SocialFilters::default(),
        read_as: ReadAs::Stream,
        count: SocialCount::Unread,
    }
}

fn view_ids(world: &World) -> Vec<String> {
    let views = world.commands.social_views().unwrap();
    views.into_iter().map(|e| e.view.id).collect()
}

fn place_ids(world: &World, view: &str) -> Vec<String> {
    let places = world.commands.social_view_places(view).unwrap();
    places.into_iter().map(|p| p.place.id).collect()
}

#[test]
fn the_built_in_views_come_first_and_all_channels_last() {
    let world = world();
    assert_eq!(
        view_ids(&world),
        [HOME, MENTIONS, MESSAGES, DRAFTS, ALL_CHANNELS]
    );
    let mine = world
        .commands
        .save_social_view(&view("Work", &[(ADA, GENERAL)], &[]))
        .unwrap();
    assert_eq!(
        view_ids(&world),
        [
            HOME,
            MENTIONS,
            MESSAGES,
            DRAFTS,
            mine.id.as_str(),
            ALL_CHANNELS
        ]
    );
    let home = world.commands.social_view(HOME).unwrap();
    assert_eq!(home.built_in, Some(BuiltInView::Home));
    assert_eq!(
        home.view.filters.kinds,
        [ContentKind::Memo, ContentKind::Article]
    );
    assert_eq!(home.original.as_ref(), Some(&home.view));
}

#[test]
fn built_in_views_show_what_they_are_for() {
    let world = world();
    // Home: the channels followed; Messages: conversations.
    assert_eq!(place_ids(&world, HOME), [GENERAL, CARL_CHANNEL]);
    assert_eq!(place_ids(&world, MESSAGES), [DM]);
    assert_eq!(place_ids(&world, MENTIONS).len(), 4);
    assert!(place_ids(&world, DRAFTS).is_empty());
    // A follow being sent shows at once.
    world.commands.follow_social_place(ADA, NEWS, true).unwrap();
    assert_eq!(place_ids(&world, HOME), [GENERAL, NEWS, CARL_CHANNEL]);
}

#[test]
fn a_built_in_view_can_be_saved_over_and_put_back() {
    let world = world();
    let mut home = world.commands.social_view(HOME).unwrap().view;
    home.name = "Front page".into();
    home.read_as = ReadAs::Stream;
    world.commands.save_social_view(&home).unwrap();
    let saved = world.commands.social_view(HOME).unwrap();
    assert_eq!(saved.view.name, "Front page");
    assert_eq!(saved.original.unwrap().name, "Home");

    let mut chosen = home.clone();
    chosen.places = vec![PlaceRef {
        account: ADA.into(),
        id: GENERAL.into(),
    }];
    assert!(refused(world.commands.save_social_view(&chosen)).contains("built in"));
    let mut drafts = world.commands.social_view(DRAFTS).unwrap().view;
    drafts.filters.kinds = vec![ContentKind::Memo];
    assert!(refused(world.commands.save_social_view(&drafts)).contains("filters don't narrow"));

    world.commands.remove_social_view(HOME).unwrap();
    assert_eq!(world.commands.social_view(HOME).unwrap().view.name, "Home");
}

#[test]
fn a_view_of_ones_own_is_checked_and_kept_tidy() {
    let world = world();
    assert!(refused(world.commands.save_social_view(&view("  ", &[], &[]))).contains("name"));
    let unknown = view(
        "Odd",
        &[(ADA, "https://groups.example/api/groups/g1/channels/none")],
        &[],
    );
    assert!(refused(world.commands.save_social_view(&unknown)).contains("knows no place"));
    let mut bad = view("Odd", &[], &[]);
    bad.filters.search = "from:".into();
    assert!(refused(world.commands.save_social_view(&bad)).contains("from:"));

    // Places named twice, in another spelling of the account, are kept
    // once; a group means all its channels, those made later too.
    let saved = world
        .commands
        .save_social_view(&view(
            " Reading ",
            &[(ADA, NEWS), ("ADA@groups.example", NEWS)],
            &[(ADA, GROUP)],
        ))
        .unwrap();
    assert_eq!(saved.name, "Reading");
    assert_eq!(saved.places.len(), 1);
    assert_eq!(place_ids(&world, &saved.id), [NEWS, GENERAL]);
    let later = "https://groups.example/api/groups/g1/channels/later";
    world
        .store
        .put_social_place(&channel(ADA, later, GROUP, "later", false))
        .unwrap();
    assert_eq!(place_ids(&world, &saved.id), [NEWS, GENERAL, later]);

    world.commands.remove_social_view(&saved.id).unwrap();
    assert!(refused(world.commands.social_view(&saved.id)).contains("no view"));
}

#[test]
fn views_of_ones_own_are_put_in_order() {
    let world = world();
    let save = |name: &str| {
        world
            .commands
            .save_social_view(&view(name, &[(ADA, GENERAL)], &[]))
            .unwrap()
            .id
    };
    let (a, b, c) = (save("A"), save("B"), save("C"));
    let mine = |world: &World| {
        let ids = view_ids(world);
        ids[4..ids.len() - 1].to_vec()
    };
    assert_eq!(mine(&world), [a.clone(), b.clone(), c.clone()]);
    world
        .commands
        .order_social_views(std::slice::from_ref(&c))
        .unwrap();
    assert_eq!(mine(&world), [c.clone(), a.clone(), b.clone()]);
    assert!(refused(world.commands.order_social_views(&[HOME.into()])).contains("no view"));
}

#[test]
fn a_tally_counts_unread_messages_as_the_view_filters_them() {
    let world = world();
    world.bob(GENERAL, "m1", 1, "hello");
    let mut memo = message(ADA, GENERAL, "m2", 2, BOB, "a memo");
    memo.kind = "memo".into();
    memo.mentions_me = true;
    world.put(&memo);
    let mut read = message(ADA, GENERAL, "m3", 3, BOB, "seen");
    read.kind = "memo".into();
    read.unread = false;
    world.put(&read);
    world.bob(DM, "d1", 4, "psst");

    let home = world.commands.social_view_tally(HOME).unwrap();
    assert_eq!((home.unread, home.mentions), (1, 1));
    assert_eq!(home.shown(SocialCount::Mentions), Some(1));
    assert_eq!(home.shown(SocialCount::None), None);
    let mentions = world.commands.social_view_tally(MENTIONS).unwrap();
    assert_eq!((mentions.unread, mentions.mentions), (1, 1));
    let messages = world.commands.social_view_tally(MESSAGES).unwrap();
    assert_eq!((messages.unread, messages.mentions), (1, 0));
    let drafts = world.commands.social_view_tally(DRAFTS).unwrap();
    assert_eq!(drafts.unread, 0);
}
