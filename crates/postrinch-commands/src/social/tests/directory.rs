//! All channels: every group an account knows, where it stands in each,
//! and their channels.

use super::*;
use crate::{SocialDirectoryGroup, SocialGroupStanding};

const ASKED: &str = "https://far.example/api/groups/asks";
const FOLLOWED: &str = "https://far.example/api/groups/open";
const FOLLOWED_CHANNEL: &str = "https://far.example/api/groups/open/channels/blog";
/// A channel followed in a group Postrinch keeps nothing of.
const LOOSE: &str = "https://loose.example/api/groups/g9/channels/c9";

fn outside(uri: &str, name: &str, policy: &str, found: GroupFound) -> SocialGroup {
    SocialGroup {
        group: json!({ "id": uri, "name": name, "joinPolicy": policy, "tier": "public" }),
        role: None,
        found,
        ..group(ADA, uri, name)
    }
}

fn adas(world: &World) -> Vec<SocialDirectoryGroup> {
    let account = postrinch_store::social_account_key(ADA);
    world
        .commands
        .social_directory()
        .unwrap()
        .into_iter()
        .filter(|g| g.account == account)
        .collect()
}

#[test]
fn all_channels_lists_every_group_and_where_the_account_stands() {
    let world = world();
    let mut home = group(ADA, GROUP, "Home group");
    home.role = Some("admin".into());
    world.store.put_social_group(&home).unwrap();
    let asked = outside(ASKED, "Asks first", "request", GroupFound::Asked);
    world.store.put_social_group(&asked).unwrap();
    let open = outside(FOLLOWED, "Open house", "open", GroupFound::Followed);
    world.store.put_social_group(&open).unwrap();
    world
        .store
        .put_social_place(&channel(ADA, FOLLOWED_CHANNEL, FOLLOWED, "blog", true))
        .unwrap();
    let mut loose = channel(ADA, LOOSE, "", "loose", true);
    loose.group = None;
    world.store.put_social_place(&loose).unwrap();

    let groups = adas(&world);
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    let loose_group = "https://loose.example/api/groups/g9";
    assert_eq!(
        names,
        ["Far group", "Home group", "Asks first", loose_group, "Open house"],
        "the account's own groups first, then by name"
    );
    let by = |uri: &str| groups.iter().find(|g| g.uri == uri).unwrap();
    assert_eq!(
        by(GROUP).standing,
        SocialGroupStanding::Member {
            role: "admin".into(),
            manage: true,
            moderate: true
        }
    );
    assert_eq!(
        by(FAR).standing,
        SocialGroupStanding::Member {
            role: "member".into(),
            manage: false,
            moderate: false
        }
    );
    assert_eq!(by(ASKED).standing, SocialGroupStanding::Asked);
    assert!(by(ASKED).channels.is_empty());
    assert_eq!(
        by(FOLLOWED).standing,
        SocialGroupStanding::Outside {
            policy: Some("open".into())
        }
    );
    let blog: Vec<&str> = by(FOLLOWED).channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(blog, ["blog"]);
    let unknown = by(loose_group);
    assert_eq!(unknown.standing, SocialGroupStanding::Outside { policy: None });
    assert!(unknown.group.is_none());
    assert_eq!(unknown.channels[0].place.id, LOOSE);
    // The conversation is not a channel.
    let every: Vec<&str> = groups
        .iter()
        .flat_map(|g| g.channels.iter().map(|c| c.place.id.as_str()))
        .collect();
    assert!(!every.contains(&DM));
    assert!(every.contains(&GENERAL) && every.contains(&NEWS));

    // The view panel's groups are only the account's own.
    let own: Vec<String> = world
        .commands
        .social_groups()
        .unwrap()
        .into_iter()
        .filter(|g| g.group.account == ADA)
        .map(|g| g.group.uri)
        .collect();
    assert_eq!(own, [FAR.to_owned(), GROUP.to_owned()]);
}
