use postrinch_commands::{SocialGroupInfo, SocialPlaceInfo};
use postrinch_store::{
    ContentKind, GroupFound, GroupRef, Notices, PlaceCursors, PlaceKind, PlaceRef, SocialGroup,
    SocialPlace,
};
use serde_json::json;

use super::*;

const ME: &str = "sam@forumall.example";
const RUST: &str = "https://forumall.example/api/groups/rust";

fn group() -> SocialGroupInfo {
    SocialGroupInfo {
        group: SocialGroup {
            account: ME.into(),
            uri: RUST.into(),
            provider: "forumall.example".into(),
            group: json!({ "name": "Rust Audio" }),
            role: Some("member".into()),
            found: GroupFound::Member,
            notices: Some(Notices::MentionsAndReplies),
        },
        name: "Rust Audio".into(),
        pending: None,
    }
}

fn channel(name: &str) -> SocialPlaceInfo {
    SocialPlaceInfo {
        place: SocialPlace {
            account: ME.into(),
            id: format!("{RUST}/channels/{name}"),
            kind: PlaceKind::Channel,
            group: Some(RUST.into()),
            followed: true,
            last_activity: None,
            object: json!({ "name": name }),
            cursors: PlaceCursors::default(),
            read_marker: None,
            notices: None,
            muted: false,
        },
        name: name.into(),
        group_name: Some("Rust Audio".into()),
        unread: 0,
        mentions: 0,
        may_post: vec![ContentKind::Chat],
        may_react: true,
        pending: None,
    }
}

fn conversation() -> SocialPlaceInfo {
    let mut info = channel("Mira");
    info.place.kind = PlaceKind::Conversation;
    info.place.group = None;
    info.place.id = "dm_mira".into();
    info.group_name = None;
    info
}

fn choices() -> Choices {
    Choices {
        groups: vec![group()],
        places: vec![channel("general"), channel("standup"), conversation()],
    }
}

fn group_ref() -> GroupRef {
    GroupRef {
        account: "Sam@ForumAll.example".into(),
        uri: RUST.into(),
    }
}

#[test]
fn chips_name_what_is_known_and_say_where() {
    let general = place_ref(&channel("general"));
    let shown = chips(&[group_ref()], std::slice::from_ref(&general), &choices());
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[0].name, "Rust Audio");
    assert_eq!(shown[0].detail, "every channel, now and later");
    assert_eq!(
        shown[0].bell,
        Some(Bell::Notices(Some(Notices::MentionsAndReplies)))
    );
    assert_eq!(shown[1].chosen, Chosen::Place(general));
    assert_eq!(shown[1].before, "Rust Audio \u{203a} ");
    assert_eq!(shown[1].name, "# general");
    assert_eq!(shown[1].detail, "forumall.example");
    assert_eq!(shown[1].bell, Some(Bell::Notices(None)));
}

#[test]
fn a_conversations_chip_offers_to_mute_it() {
    let shown = chips(&[], &[place_ref(&conversation())], &choices());
    assert_eq!(shown[0].bell, Some(Bell::Muted(false)));
    let mut muted = conversation();
    muted.place.muted = true;
    let choices = Choices {
        groups: vec![],
        places: vec![muted.clone()],
    };
    let shown_muted = chips(&[], &[place_ref(&muted)], &choices);
    assert_eq!(shown_muted[0].bell, Some(Bell::Muted(true)));
    assert_eq!(shown[0].before, "");
    assert_eq!(shown[0].name, "\u{2709} Mira");
}

#[test]
fn a_place_not_known_yet_shows_by_what_the_view_saved() {
    let unknown = PlaceRef {
        account: "ada@sonic.example".into(),
        id: "https://sonic.example/c/9".into(),
    };
    let shown = chips(&[], std::slice::from_ref(&unknown), &Choices::default());
    assert_eq!(shown[0].name, unknown.id);
    assert_eq!(shown[0].detail, "sonic.example");
    assert_eq!(shown[0].bell, None);
}

#[test]
fn offers_leave_out_what_the_view_has() {
    let general = place_ref(&channel("general"));
    let offered = offers(&[group_ref()], &[general], &choices(), "");
    let names: Vec<&str> = offered.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["# standup", "\u{2709} Mira"]);
}

#[test]
fn a_groups_offer_says_its_provider() {
    let offered = offers(&[], &[], &choices(), "");
    assert_eq!(offered[0].name, "Rust Audio");
    assert_eq!(offered[0].detail, "every channel \u{b7} forumall.example");
}

#[test]
fn offers_match_what_is_typed_in_name_group_or_provider() {
    let names = |typed: &str| -> Vec<String> {
        offers(&[], &[], &choices(), typed)
            .into_iter()
            .map(|o| o.name)
            .collect()
    };
    assert_eq!(names("  STAND "), ["# standup"]);
    assert_eq!(names("rust audio").len(), 3);
    assert_eq!(names("forumall").len(), 4);
    assert!(names("nowhere").is_empty());
}
