//! Notices: how each place is heard (everything, mentions and replies,
//! nothing; a conversation unless muted), the rail's count, the views'
//! badges and the new messages a desktop notice tells.

use postrinch_store::{
    Notices, PlaceRef, ReadAs, SocialCount, SocialFilters, SocialPlace, SocialView,
};

use super::*;
use crate::{ALL_CHANNELS, Heard, MENTIONS, MESSAGES, Notice, PROVIDER_DEFAULT, heard, notice};

/// A view of Ada's of `places`.
fn view_of(name: &str, places: &[&str]) -> SocialView {
    SocialView {
        id: String::new(),
        order: 0,
        name: name.into(),
        places: places
            .iter()
            .map(|id| PlaceRef {
                account: ADA.into(),
                id: (*id).into(),
            })
            .collect(),
        groups: vec![],
        filters: SocialFilters::default(),
        read_as: ReadAs::Stream,
        count: SocialCount::Unread,
    }
}

/// A message from Bob that mentions Ada.
fn mention(world: &World, place: &str, id: &str, created: i64) {
    let mut m = message(ADA, place, id, created, BOB, "@ada look");
    m.mentions_me = true;
    world.put(&m);
}

/// Set a channel's notices as kept, as sync would.
fn kept_notices(world: &World, place: &str, notices: Option<Notices>) {
    world
        .store
        .change_social_place(ADA, place, |p| p.notices = notices)
        .unwrap();
}

#[test]
fn what_gets_through_follows_how_a_place_is_heard() {
    assert_eq!(notice(Heard::Every, false), Notice::Told);
    assert_eq!(notice(Heard::Every, true), Notice::Called);
    assert_eq!(notice(Heard::MentionsAndReplies, false), Notice::Quiet);
    assert_eq!(notice(Heard::MentionsAndReplies, true), Notice::Called);
    assert_eq!(notice(Heard::Nothing, false), Notice::Quiet);
    assert_eq!(notice(Heard::Nothing, true), Notice::Quiet);
}

#[test]
fn a_channel_is_heard_by_its_own_setting_then_its_groups_then_the_default() {
    let mut place = channel(ADA, GENERAL, GROUP, "general", true);
    place.notices = Some(Notices::Nothing);
    assert_eq!(heard(&place, Some(Notices::Everything)), Heard::Nothing);
    place.notices = None;
    assert_eq!(heard(&place, Some(Notices::Everything)), Heard::Every);
    assert_eq!(
        heard(&place, None),
        heard(&channel_with(PROVIDER_DEFAULT), None)
    );
    assert_eq!(heard(&place, None), Heard::MentionsAndReplies);
}

fn channel_with(notices: Notices) -> SocialPlace {
    let mut place = channel(ADA, GENERAL, GROUP, "general", true);
    place.notices = Some(notices);
    place
}

#[test]
fn a_conversation_is_always_heard_unless_muted() {
    let mut dm = conversation(ADA, DM);
    // A group's setting or a stray notices value never reaches it.
    dm.notices = Some(Notices::Nothing);
    assert_eq!(heard(&dm, Some(Notices::Nothing)), Heard::Every);
    dm.muted = true;
    assert_eq!(heard(&dm, None), Heard::Nothing);
}

#[test]
fn the_rail_counts_what_gets_through_each_places_notices() {
    let world = world();
    // general is "mentions and replies": a plain message is quiet.
    world.bob(GENERAL, "g1", 1, "hello all");
    mention(&world, GENERAL, "g2", 2);
    world.bob(DM, "d1", 3, "psst");
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 2);

    // Everything, as the person sets it: shown at once, before sync.
    world
        .commands
        .set_social_place_notices(ADA, GENERAL, Some(Notices::Everything))
        .unwrap();
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 3);

    world
        .commands
        .set_social_place_notices(ADA, GENERAL, Some(Notices::Nothing))
        .unwrap();
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 1);

    world
        .commands
        .mute_social_conversation(ADA, DM, true)
        .unwrap();
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 0);
    world
        .commands
        .mute_social_conversation(ADA, DM, false)
        .unwrap();
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 1);
}

#[test]
fn a_channel_without_a_setting_is_heard_as_its_group() {
    let world = world();
    kept_notices(&world, NEWS, None);
    world.bob(NEWS, "n1", 1, "news");
    // The provider's default: mentions and replies.
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 0);
    world
        .commands
        .set_social_group_notices(ADA, FAR, Some(Notices::Everything))
        .unwrap();
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 1);

    // A group not kept here sets nothing: the provider's default again.
    world
        .store
        .change_social_place(ADA, NEWS, |p| p.group = Some("https://nowhere/g".into()))
        .unwrap();
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 0);
    mention(&world, NEWS, "n2", 2);
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 1);
}

#[test]
fn a_reply_to_the_person_gets_through_mentions_and_replies() {
    let world = world();
    let mut own = message(ADA, GENERAL, "mine", 1, ADA, "my question");
    own.unread = false;
    world.put(&own);
    let mut reply = message(ADA, GENERAL, "r1", 2, BOB, "an answer");
    reply.parent = Some("mine".into());
    world.put(&reply);
    let mut other = message(ADA, GENERAL, "r2", 3, BOB, "an aside");
    other.parent = Some("g0".into());
    world.put(&other);
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 1);
}

#[test]
fn read_deleted_and_own_messages_are_never_counted() {
    let world = world();
    let mut read = message(ADA, DM, "d1", 1, BOB, "seen");
    read.unread = false;
    world.put(&read);
    let mut gone = message(ADA, DM, "d2", 2, BOB, "gone");
    gone.deleted = true;
    world.put(&gone);
    world.put(&message(ADA, DM, "d3", 3, ADA, "mine"));
    assert_eq!(world.commands.social_notices(0, 0).unwrap().noticed, 0);
}

#[test]
fn muting_is_for_conversations_and_stays_on_this_computer() {
    let world = world();
    assert!(
        refused(world.commands.mute_social_conversation(ADA, GENERAL, true)).contains("channel")
    );
    let dm = world
        .commands
        .mute_social_conversation(ADA, DM, true)
        .unwrap();
    assert!(dm.place.muted);
    assert!(world.store.social_place(ADA, DM).unwrap().unwrap().muted);
    assert!(world.commands.social_outbox(ADA).unwrap().is_empty());
}

#[test]
fn a_place_kept_before_muting_existed_reads_as_heard() {
    let mut json = serde_json::to_value(conversation(ADA, DM)).unwrap();
    json.as_object_mut().unwrap().remove("muted");
    let place: SocialPlace = serde_json::from_value(json).unwrap();
    assert!(!place.muted);
}

#[test]
fn a_views_count_is_a_badge_for_what_calls_for_the_person() {
    let world = world();
    world.bob(GENERAL, "g1", 1, "hello all");
    mention(&world, GENERAL, "g2", 2);
    world.bob(DM, "d1", 3, "psst");
    let work = world
        .commands
        .save_social_view(&view_of("Work", &[GENERAL]))
        .unwrap();
    let work = world.commands.social_view_tally(&work.id).unwrap();
    assert_eq!((work.unread, work.called), (2, 1));
    let messages = world.commands.social_view_tally(MESSAGES).unwrap();
    assert_eq!((messages.unread, messages.called), (1, 1));

    // Muted, the conversation still counts as unread, but quietly.
    world
        .commands
        .mute_social_conversation(ADA, DM, true)
        .unwrap();
    let messages = world.commands.social_view_tally(MESSAGES).unwrap();
    assert_eq!((messages.unread, messages.called), (1, 0));
    // A mention in a channel heard as nothing is quiet too.
    world
        .commands
        .set_social_place_notices(ADA, GENERAL, Some(Notices::Nothing))
        .unwrap();
    let mentions = world.commands.social_view_tally(MENTIONS).unwrap();
    assert_eq!((mentions.unread, mentions.called), (1, 0));
}

#[test]
fn new_messages_are_told_newest_first_with_where_they_open() {
    let world = world();
    world.bob(GENERAL, "old", 1_000, "before the window opened");
    mention(&world, GENERAL, "g2", 5_000);
    world.bob(GENERAL, "g3", 6_000, "quiet in mentions and replies");
    world.bob(DM, "d1", 7_000, "**psst**, the config");
    kept_notices(&world, NEWS, Some(Notices::Everything));
    world.bob(NEWS, "n1", 8_000, "news for everyone");

    let told = world.commands.social_notices(2_000, 20).unwrap().arrived;
    let ids: Vec<&str> = told.iter().map(|t| t.message.as_str()).collect();
    assert_eq!(ids, ["n1", "d1", "g2"]);
    let (news, dm, at_me) = (&told[0], &told[1], &told[2]);
    assert_eq!(news.view, ALL_CHANNELS, "not followed, in no view");
    assert!(!news.called);
    assert_eq!(news.group_name.as_deref(), Some("Far group"));
    assert_eq!(dm.view, MESSAGES);
    assert!(dm.conversation && dm.called);
    assert_eq!(dm.place_name, BOB);
    assert_eq!(dm.snippet, "psst, the config");
    assert_eq!(at_me.view, MENTIONS);
    assert_eq!(at_me.place_name, "general");

    // A limit keeps the newest, across places; the count is of all that
    // get through, whenever made.
    let few = world.commands.social_notices(2_000, 1).unwrap();
    let ids: Vec<&str> = few.arrived.iter().map(|t| t.message.as_str()).collect();
    assert_eq!(ids, ["n1"]);
    assert_eq!(few.noticed, 3);
    // Asking for none tells none, and still counts.
    let none = world.commands.social_notices(2_000, 0).unwrap();
    assert!(none.arrived.is_empty());
    assert_eq!(none.noticed, 3);
}

#[test]
fn a_new_message_opens_in_the_persons_own_view_that_shows_it() {
    let world = world();
    let saved = world
        .commands
        .save_social_view(&view_of("Far", &[NEWS]))
        .unwrap();
    kept_notices(&world, NEWS, Some(Notices::Everything));
    world.bob(NEWS, "n1", 5, "news");
    let told = world.commands.social_notices(0, 20).unwrap().arrived;
    assert_eq!(told[0].view, saved.id);
    assert_eq!(told[0].place.id, NEWS);
}

#[test]
fn an_agent_is_told_only_of_the_accounts_it_sees() {
    let world = world();
    world.bob(DM, "d1", 3, "psst");
    let carl = world.commands.limited(&[], &[CARL]);
    assert_eq!(carl.social_notices(0, 0).unwrap().noticed, 0);
    assert!(carl.social_notices(0, 20).unwrap().arrived.is_empty());
}
