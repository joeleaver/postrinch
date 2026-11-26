//! Places and groups, read state, follows and notices, the profile, and
//! rendering a message.

use postrinch_store::{
    AvatarChange, DraftMedia, Notices, ProfileChange, SocialChange, SocialMedia,
};

use super::*;
use crate::SocialAvatar;

/// The read changes queued for the provider.
fn reads(world: &World) -> Vec<String> {
    let outbox = world.commands.social_outbox(ADA).unwrap();
    outbox
        .into_iter()
        .filter_map(|e| match e.change {
            SocialChange::Read { message } => Some(message),
            _ => None,
        })
        .collect()
}

#[test]
fn places_and_groups_are_listed_with_what_they_hold() {
    let world = world();
    world.bob(GENERAL, "m1", 1, "hi");
    let mut mention = message(ADA, GENERAL, "m2", 2, BOB, "@ada");
    mention.mentions_me = true;
    world.put(&mention);
    let general = world.commands.social_place(ADA, GENERAL).unwrap();
    assert_eq!(general.name, "general");
    assert_eq!(general.group_name.as_deref(), Some("Home group"));
    assert_eq!((general.unread, general.mentions), (2, 1));
    assert!(general.may_react);
    let places = world.commands.social_places().unwrap();
    assert_eq!(places.len(), 4);
    let groups: Vec<String> = world
        .commands
        .social_groups()
        .unwrap()
        .into_iter()
        .map(|g| g.name)
        .collect();
    assert_eq!(groups, ["Far group", "Home group", "Carl's group"]);
}

#[test]
fn reading_a_home_channel_moves_its_marker_forward_only() {
    let world = world();
    for (id, at) in [("a", 1), ("b", 2), ("c", 3)] {
        world.bob(GENERAL, id, at, id);
    }
    assert_eq!(
        world
            .commands
            .mark_social_read(ADA, GENERAL, Some("b"))
            .unwrap(),
        2
    );
    assert_eq!(world.commands.social_place(ADA, GENERAL).unwrap().unread, 1);
    assert_eq!(reads(&world), ["b"]);
    let marker = |w: &World| {
        w.store
            .social_place(ADA, GENERAL)
            .unwrap()
            .unwrap()
            .read_marker
    };
    assert_eq!(marker(&world).as_deref(), Some("b"));

    // Marking an earlier one moves nothing back.
    world
        .commands
        .mark_social_unread(ADA, GENERAL, "a")
        .unwrap();
    assert_eq!(
        world
            .commands
            .mark_social_read(ADA, GENERAL, Some("a"))
            .unwrap(),
        1
    );
    assert_eq!(marker(&world).as_deref(), Some("b"));
    assert_eq!(reads(&world), ["b"]);

    assert_eq!(
        world.commands.mark_social_read(ADA, GENERAL, None).unwrap(),
        1
    );
    assert_eq!(marker(&world).as_deref(), Some("c"));
    assert_eq!(reads(&world), ["b", "c"]);
    assert_eq!(
        world.commands.mark_social_read(ADA, GENERAL, None).unwrap(),
        0
    );
    assert!(
        refused(world.commands.mark_social_read(ADA, GENERAL, Some("z"))).contains("no message z")
    );
    assert!(refused(world.commands.mark_social_unread(ADA, GENERAL, "z")).contains("no message z"));
}

#[test]
fn reading_elsewhere_is_kept_here_alone() {
    let world = world();
    world.bob(NEWS, "n1", 1, "news");
    world.bob(DM, "d1", 1, "psst");
    assert_eq!(world.commands.mark_social_read(ADA, NEWS, None).unwrap(), 1);
    assert_eq!(world.commands.mark_social_read(ADA, DM, None).unwrap(), 1);
    assert_eq!(world.commands.social_place(ADA, NEWS).unwrap().unread, 0);
    assert!(reads(&world).is_empty());
}

#[test]
fn a_follow_shows_at_once_and_is_queued_once() {
    let world = world();
    let followed = world.commands.follow_social_place(ADA, NEWS, true).unwrap();
    assert!(followed.place.followed);
    assert!(followed.pending.is_some());
    world.commands.follow_social_place(ADA, NEWS, true).unwrap();
    let outbox = world.commands.social_outbox(ADA).unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(
        outbox[0].change,
        SocialChange::Follow {
            group: Some(FAR.into()),
            on: true,
        }
    );
    assert!(
        !world
            .commands
            .follow_social_place(ADA, NEWS, false)
            .unwrap()
            .place
            .followed
    );
    assert!(refused(world.commands.follow_social_place(ADA, DM, false)).contains("Only channels"));
}

#[test]
fn notices_are_set_on_a_channel_or_a_group() {
    let world = world();
    let general = world
        .commands
        .set_social_place_notices(ADA, GENERAL, Some(Notices::Everything))
        .unwrap();
    assert_eq!(general.place.notices, Some(Notices::Everything));
    world
        .commands
        .set_social_place_notices(ADA, GENERAL, Some(Notices::Everything))
        .unwrap();
    assert_eq!(world.commands.social_outbox(ADA).unwrap().len(), 1);
    assert!(
        refused(world.commands.set_social_place_notices(ADA, DM, None))
            .contains("always tells you")
    );

    let group = world
        .commands
        .set_social_group_notices(ADA, FAR, Some(Notices::Nothing))
        .unwrap();
    assert_eq!(group.group.notices, Some(Notices::Nothing));
    assert!(group.pending.is_some());
    assert!(
        refused(world.commands.set_social_group_notices(
            ADA,
            "https://x.example/api/groups/q",
            None
        ))
        .contains("knows no group")
    );
}

#[test]
fn a_profile_change_shows_at_once() {
    let world = world();
    let before = world.commands.social_account(ADA).unwrap();
    assert_eq!(before.display_name.as_deref(), Some("Ada"));
    assert_eq!(before.avatar, SocialAvatar::None);
    assert!(
        refused(
            world
                .commands
                .change_social_profile(ADA, &ProfileChange::default())
        )
        .contains("Nothing to change")
    );
    let blank = ProfileChange {
        display_name: Some("  ".into()),
        ..ProfileChange::default()
    };
    assert!(refused(world.commands.change_social_profile(ADA, &blank)).contains("can't be empty"));
    let text = world.store.put_social_media_bytes(b"text").unwrap();
    let not_picture = ProfileChange {
        avatar: Some(AvatarChange::Set {
            media: DraftMedia::Kept {
                blob: text,
                name: "a.txt".into(),
                mime: None,
            },
        }),
        ..ProfileChange::default()
    };
    assert!(
        refused(world.commands.change_social_profile(ADA, &not_picture))
            .contains("must be a picture")
    );

    let picture = DraftMedia::Kept {
        blob: world.store.put_social_media_bytes(b"\x89PNG").unwrap(),
        name: "me.png".into(),
        mime: None,
    };
    let change = ProfileChange {
        display_name: Some(" Ada L. ".into()),
        bio: Some("Counting".into()),
        avatar: Some(AvatarChange::Set {
            media: picture.clone(),
        }),
    };
    let after = world.commands.change_social_profile(ADA, &change).unwrap();
    assert_eq!(after.display_name.as_deref(), Some("Ada L."));
    assert_eq!(after.bio.as_deref(), Some("Counting"));
    assert_eq!(after.avatar, SocialAvatar::Pending { media: picture });
    assert!(after.pending.is_some());
    assert_eq!(world.commands.social_accounts().unwrap().len(), 2);
}

#[test]
fn a_message_renders_with_its_fetched_pictures() {
    let world = world();
    let mut message = message(ADA, GENERAL, "m1", 1, BOB, "Look ![a](attachment:p1) @carl");
    let url = "https://groups.example/media/p1.png";
    message.message["attachments"] = json!([
        { "id": "p1", "mime": "image/png", "url": url, "filename": "p1.png" }
    ]);
    world.put(&message);
    let rendered = world.commands.render_social(&message, false).unwrap();
    assert_eq!(rendered.placed, ["p1"]);
    assert!(
        rendered
            .mentions
            .contains(&"carl@groups.example".to_owned())
    );

    let blob = world.store.put_social_media_bytes(b"\x89PNG").unwrap();
    world
        .store
        .put_social_media(&SocialMedia {
            account: ADA.into(),
            provider: "groups.example".into(),
            id: "p1".into(),
            url: url.into(),
            blob: Some(blob),
            fetched: Some(1),
            mime: Some("image/png".into()),
            size: Some(4),
            width: None,
            height: None,
            duration_ms: None,
            poster: None,
        })
        .unwrap();
    let rendered = world.commands.render_social(&message, false).unwrap();
    assert!(
        rendered.html.contains("data:image/png;base64,"),
        "{}",
        rendered.html
    );
}

#[test]
fn a_marker_at_a_message_not_kept_here_is_not_moved() {
    let world = world();
    world.bob(GENERAL, "a", 1, "a");
    // Read elsewhere, up to a message sync has not fetched yet.
    world
        .store
        .change_social_place(ADA, GENERAL, |p| p.read_marker = Some("unfetched".into()))
        .unwrap();
    assert_eq!(
        world.commands.mark_social_read(ADA, GENERAL, None).unwrap(),
        1
    );
    let place = world.store.social_place(ADA, GENERAL).unwrap().unwrap();
    assert_eq!(place.read_marker.as_deref(), Some("unfetched"));
    assert!(reads(&world).is_empty());
}
