use postrinch_commands::{BuiltInView, SocialPlaceInfo, SocialViewEntry, SocialViewTally};
use postrinch_store::{
    ContentKind, Notices, PlaceCursors, PlaceKind, ReadAs, SocialCount, SocialPlace,
};
use serde_json::{Value, json};

use super::*;

const ME: &str = "sam@forumall.example";
const RUST: &str = "https://forumall.example/api/groups/rust";
const SPEC: &str = "https://ofscp.example/api/groups/spec";

fn channel(group: &str, group_name: &str, name: &str, object: Value) -> SocialPlaceInfo {
    let mut object = object;
    object["name"] = json!(name);
    SocialPlaceInfo {
        place: SocialPlace {
            account: ME.into(),
            id: format!("{group}/channels/{name}"),
            kind: PlaceKind::Channel,
            group: Some(group.into()),
            followed: true,
            last_activity: None,
            object,
            cursors: PlaceCursors::default(),
            read_marker: None,
            notices: None,
        },
        name: name.into(),
        group_name: Some(group_name.into()),
        unread: 0,
        mentions: 0,
        may_post: vec![ContentKind::Chat],
        may_react: true,
        pending: None,
    }
}

fn conversation(with: &str) -> SocialPlaceInfo {
    let mut info = channel(RUST, "", with, json!({}));
    info.place.kind = PlaceKind::Conversation;
    info.place.group = None;
    info.place.id = format!("dm_{with}");
    info.group_name = None;
    info
}

fn own(read_as: ReadAs) -> SocialViewEntry {
    let mut view = BuiltInView::Mentions.original();
    view.id = "v1".into();
    view.name = "Mine".into();
    view.read_as = read_as;
    SocialViewEntry {
        view,
        built_in: None,
        original: None,
    }
}

fn built_in(kind: BuiltInView) -> SocialViewEntry {
    SocialViewEntry {
        view: kind.original(),
        built_in: Some(kind),
        original: Some(kind.original()),
    }
}

fn tally(unread: usize, mentions: usize) -> SocialViewTally {
    SocialViewTally {
        id: "v1".into(),
        unread,
        mentions,
    }
}

#[test]
fn built_in_views_have_their_own_glyphs() {
    let glyphs: Vec<Mark> = [
        BuiltInView::Home,
        BuiltInView::Mentions,
        BuiltInView::Messages,
        BuiltInView::Drafts,
        BuiltInView::AllChannels,
    ]
    .into_iter()
    .map(|k| view_mark(&built_in(k), &[]))
    .collect();
    assert_eq!(
        glyphs,
        ["\u{2302}", "@", "\u{2709}", "\u{270e}", "#"].map(Mark::Glyph)
    );
}

#[test]
fn a_view_is_marked_by_its_groups_or_as_a_list() {
    let rust = GroupMark {
        uri: RUST.into(),
        name: "Rust Audio".into(),
    };
    let spec = GroupMark {
        uri: SPEC.into(),
        name: "OFSCP Spec".into(),
    };
    let stream = own(ReadAs::Stream);
    assert_eq!(view_mark(&stream, &[]), Mark::Glyph("#"));
    assert_eq!(
        view_mark(&stream, std::slice::from_ref(&rust)),
        Mark::Group(rust.clone())
    );
    assert_eq!(
        view_mark(&stream, &[spec.clone(), rust.clone()]),
        Mark::Stack(vec![spec, rust.clone()])
    );
    assert_eq!(
        view_mark(&own(ReadAs::List), &[rust]),
        Mark::Glyph("\u{25a4}")
    );
}

#[test]
fn a_groups_square_has_its_initial_and_the_same_colour_every_time() {
    let rust = GroupMark {
        uri: RUST.into(),
        name: "rust Audio".into(),
    };
    assert_eq!(rust.initial(), "R");
    assert_eq!(rust.color(), rust.clone().color());
    let odd = GroupMark {
        uri: SPEC.into(),
        name: "  ".into(),
    };
    assert_eq!(odd.initial(), "#");
}

#[test]
fn groups_are_listed_once_in_the_places_order() {
    let places = [
        channel(SPEC, "OFSCP Spec", "chat", json!({})),
        channel(RUST, "Rust Audio", "general", json!({})),
        channel(SPEC, "OFSCP Spec", "proposals", json!({})),
        conversation("mira@forumall.example"),
    ];
    let names: Vec<String> = groups_of(&places).into_iter().map(|g| g.name).collect();
    assert_eq!(names, ["OFSCP Spec", "Rust Audio"]);
}

#[test]
fn a_count_shows_as_the_view_asks_and_stands_out_for_mentions_and_messages() {
    let mine = own(ReadAs::Stream);
    assert_eq!(row_count(&mine, None), None);
    assert_eq!(row_count(&mine, Some(&tally(0, 0))), None);
    assert_eq!(row_count(&mine, Some(&tally(4, 0))), Some((4, false)));
    assert_eq!(row_count(&mine, Some(&tally(3, 1))), Some((3, true)));
    let messages = built_in(BuiltInView::Messages);
    assert_eq!(row_count(&messages, Some(&tally(1, 0))), Some((1, true)));
    let mut mentions_only = own(ReadAs::Stream);
    mentions_only.view.count = SocialCount::Mentions;
    assert_eq!(
        row_count(&mentions_only, Some(&tally(5, 2))),
        Some((2, true))
    );
    let mut no_count = own(ReadAs::Stream);
    no_count.view.count = SocialCount::None;
    assert_eq!(row_count(&no_count, Some(&tally(5, 2))), None);
}

#[test]
fn the_line_under_a_view_says_what_it_gathers() {
    let rust = [
        channel(RUST, "Rust Audio", "general", json!({})),
        channel(RUST, "Rust Audio", "announcements", json!({})),
    ];
    assert_eq!(sub_line(&own(ReadAs::Stream), &rust), "forumall.example");
    let mixed = [
        channel(SPEC, "OFSCP Spec", "chat", json!({})),
        channel(SPEC, "OFSCP Spec", "proposals", json!({})),
        channel(RUST, "Rust Audio", "ofscp", json!({})),
    ];
    assert_eq!(
        sub_line(&own(ReadAs::Stream), &mixed),
        "3 places on 2 providers"
    );
    let mut articles = own(ReadAs::List);
    articles.view.filters.kinds = vec![ContentKind::Article];
    assert_eq!(sub_line(&articles, &mixed), "articles from 3 places");
    assert_eq!(sub_line(&own(ReadAs::Stream), &[]), "");
}

#[test]
fn a_view_with_one_place_says_one() {
    let one = [conversation("mira@forumall.example")];
    assert_eq!(
        sub_line(&own(ReadAs::Stream), &one),
        "1 place on 1 provider"
    );
    assert_eq!(
        sub_line(&built_in(BuiltInView::Home), &one),
        "posts from 1 followed channel"
    );
}

#[test]
fn home_counts_its_followed_channels_and_other_built_in_views_say_nothing() {
    let places = [
        channel(RUST, "Rust Audio", "general", json!({})),
        channel(SPEC, "OFSCP Spec", "chat", json!({})),
    ];
    assert_eq!(
        sub_line(&built_in(BuiltInView::Home), &places),
        "posts from 2 followed channels"
    );
    assert_eq!(sub_line(&built_in(BuiltInView::Mentions), &places), "");
    assert_eq!(sub_line(&built_in(BuiltInView::Messages), &places), "");
}

#[test]
fn a_conversation_is_on_its_accounts_provider() {
    assert_eq!(
        place_provider(&conversation("mira@sonic.example")),
        "forumall.example"
    );
    assert_eq!(account_domain("Sam@ForumAll.Example"), "forumall.example");
}

#[test]
fn tabs_show_for_streams_of_several_places_and_say_where_across_groups() {
    let one_group = [
        channel(RUST, "Rust Audio", "general", json!({})),
        channel(RUST, "Rust Audio", "standup", json!({})),
    ];
    assert!(shows_tabs(&own(ReadAs::Stream), &one_group));
    assert!(!shows_tabs(&own(ReadAs::List), &one_group));
    assert!(!shows_tabs(&own(ReadAs::Stream), &one_group[..1]));
    assert!(!tabs_say_where(&one_group));
    let two = [
        channel(RUST, "Rust Audio", "general", json!({})),
        channel(SPEC, "OFSCP Spec", "chat", json!({})),
    ];
    assert!(tabs_say_where(&two));
}

#[test]
fn a_places_glyph_says_what_kind_of_place_it_is() {
    assert_eq!(place_glyph(&channel(RUST, "R", "general", json!({}))), "#");
    assert_eq!(
        place_glyph(&channel(RUST, "R", "standup", json!({ "type": "call" }))),
        "\u{25d6}"
    );
    let blog =
        json!({ "type": "text", "permissions": { "post:message": [], "post:article": ["admin"] } });
    assert_eq!(place_glyph(&channel(RUST, "R", "blog", blog)), "\u{25a4}");
    let admins_chat = json!({ "permissions": { "post:message": ["admin"] } });
    assert_eq!(place_glyph(&channel(RUST, "R", "mods", admins_chat)), "#");
    assert_eq!(
        place_glyph(&conversation("mira@forumall.example")),
        "\u{2709}"
    );
}

#[test]
fn notices_are_named_for_the_chip_and_offered_in_order() {
    assert_eq!(notices_name(Some(Notices::Everything), false), "everything");
    assert_eq!(
        notices_name(Some(Notices::MentionsAndReplies), true),
        "mentions and replies"
    );
    assert_eq!(notices_name(Some(Notices::Nothing), false), "nothing");
    assert_eq!(notices_name(None, false), "as its group");
    assert_eq!(notices_name(None, true), "the provider's default");
    let names: Vec<&str> = notices_choices(false).iter().map(|c| c.1).collect();
    assert_eq!(
        names,
        [
            "Everything",
            "Mentions and replies",
            "Nothing",
            "As its group"
        ]
    );
    assert_eq!(notices_choices(true)[3], (None, "The provider's default"));
}

#[test]
fn each_built_in_view_says_what_it_holds() {
    assert_eq!(
        built_in_holds(BuiltInView::Home),
        "Posts from the channels you follow."
    );
    assert_eq!(
        built_in_holds(BuiltInView::AllChannels),
        "Every place Postrinch knows."
    );
}

#[test]
fn a_view_naming_whole_groups_is_marked_by_them_alone() {
    let places = [
        channel(RUST, "Rust Audio", "general", json!({})),
        channel(SPEC, "OFSCP Spec", "proposals", json!({})),
    ];
    let names = |entry: &SocialViewEntry| -> Vec<String> {
        marking_groups(entry, &places)
            .into_iter()
            .map(|g| g.name)
            .collect()
    };
    let mut of_rust = own(ReadAs::Stream);
    assert_eq!(names(&of_rust), ["Rust Audio", "OFSCP Spec"]);
    of_rust.view.groups = vec![postrinch_store::GroupRef {
        account: ME.into(),
        uri: RUST.into(),
    }];
    assert_eq!(names(&of_rust), ["Rust Audio"]);
}

#[test]
fn built_in_views_have_no_tabs() {
    let places = [
        conversation("mira@forumall.example"),
        conversation("ada@forumall.example"),
    ];
    assert!(!shows_tabs(&built_in(BuiltInView::Messages), &places));
    assert!(!shows_tabs(&built_in(BuiltInView::Mentions), &places));
}

#[test]
fn a_tab_stays_chosen_only_while_it_is_one_of_the_views_tabs() {
    let places = [
        channel(RUST, "Rust Audio", "general", json!({})),
        channel(RUST, "Rust Audio", "standup", json!({})),
    ];
    let general = crate::social_ui::place_ref(&places[0]);
    let stream = own(ReadAs::Stream);
    assert_eq!(chosen_tab(&stream, &places, None), None);
    assert_eq!(
        chosen_tab(&stream, &places, Some(&general)),
        Some(general.clone())
    );
    // Taken out of the view: back to All.
    assert_eq!(chosen_tab(&stream, &places[1..], Some(&general)), None);
    // Read as a list, the view has no tabs.
    assert_eq!(
        chosen_tab(&own(ReadAs::List), &places, Some(&general)),
        None
    );
}
