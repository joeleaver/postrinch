//! All channels (docs/ofscp.md, "Organizing: views, not providers"):
//! every group Postrinch knows of, with where the account stands in it,
//! and the channels kept of it: the person's groups, groups asked to join,
//! and the groups of channels followed without joining. Private
//! conversations are not here.

use std::collections::BTreeMap;

use postrinch_store::{GroupFound, PlaceKind, SocialGroup, social_account_key};
use serde::{Deserialize, Serialize};

use super::permissions::Standing;
use super::places::{SocialGroupInfo, SocialPlaceInfo, group_info};
use super::str_at;
use crate::{CommandResult, Commands};

/// Where an account stands in a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SocialGroupStanding {
    /// In it, as `role`; whether it may manage the group (its invites) and
    /// moderate it (its join requests, as managers may too).
    Member {
        role: String,
        manage: bool,
        moderate: bool,
    },
    /// It asked to join; the request waits for the group's managers.
    Asked,
    /// Not in it. `policy` is how the group takes members (§5.7: `open`,
    /// `request` or `invite`), when Postrinch knows the group.
    Outside { policy: Option<String> },
}

/// A group as All channels lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialDirectoryGroup {
    /// The account (its store key).
    pub account: String,
    /// The group's URI.
    pub uri: String,
    /// Its name; its URI when Postrinch does not know the group itself.
    pub name: String,
    pub standing: SocialGroupStanding,
    /// The group as kept, when it is.
    pub group: Option<SocialGroupInfo>,
    /// Its channels Postrinch keeps, by name.
    pub channels: Vec<SocialPlaceInfo>,
}

impl Commands {
    /// Every group of every account this caller sees, as All channels
    /// lists them: by account, the account's own groups first, then by
    /// name.
    pub fn social_directory(&self) -> CommandResult<Vec<SocialDirectoryGroup>> {
        let mut out = Vec::new();
        for account in self.store.social_accounts()? {
            if !self.sees_social(&account.actor) {
                continue;
            }
            let outbox = self.store.social_outbox(&account.actor)?;
            let key = social_account_key(&account.actor);
            let mut groups: BTreeMap<String, SocialDirectoryGroup> = BTreeMap::new();
            for group in self.store.social_groups(&account.actor)? {
                let uri = group.uri.clone();
                let standing = standing(&group);
                let info = group_info(group, &outbox);
                groups.insert(
                    uri.clone(),
                    SocialDirectoryGroup {
                        account: key.clone(),
                        uri,
                        name: info.name.clone(),
                        standing,
                        group: Some(info),
                        channels: Vec::new(),
                    },
                );
            }
            for place in self.places_by_name(&account.actor)? {
                if place.kind != PlaceKind::Channel {
                    continue;
                }
                let Some(uri) = place
                    .group
                    .clone()
                    .or_else(|| group_of_channel(&place.id))
                else {
                    continue;
                };
                let info = self.place_info(place)?;
                groups
                    .entry(uri.clone())
                    .or_insert_with(|| SocialDirectoryGroup {
                        account: key.clone(),
                        name: uri.clone(),
                        uri,
                        standing: SocialGroupStanding::Outside { policy: None },
                        group: None,
                        channels: Vec::new(),
                    })
                    .channels
                    .push(info);
            }
            let mut groups: Vec<SocialDirectoryGroup> = groups.into_values().collect();
            groups.sort_by_cached_key(|g| {
                let theirs = matches!(g.standing, SocialGroupStanding::Member { .. });
                (!theirs, g.name.to_lowercase())
            });
            out.extend(groups);
        }
        Ok(out)
    }
}

/// Where a group's account stands in it.
fn standing(group: &SocialGroup) -> SocialGroupStanding {
    match group.found {
        GroupFound::Member => {
            let standing = Standing::in_group(group);
            SocialGroupStanding::Member {
                role: group.role.clone().unwrap_or_default(),
                manage: standing.may_manage(),
                moderate: standing.may_moderate(),
            }
        }
        GroupFound::Asked => SocialGroupStanding::Asked,
        GroupFound::Invite | GroupFound::Directory | GroupFound::Followed => {
            SocialGroupStanding::Outside {
                policy: Some(join_policy(&group.group)),
            }
        }
    }
}

/// How a group takes members (§5.7): its `joinPolicy`, but `invite` for a
/// `private` group whatever it says (§11), and `invite` when it names
/// none.
fn join_policy(group: &serde_json::Value) -> String {
    if str_at(group, "tier") == Some("private") {
        return "invite".to_owned();
    }
    str_at(group, "joinPolicy").unwrap_or("invite").to_owned()
}

/// The URI of the group a channel URI is in:
/// `https://{a}/api/groups/{g}/channels/{c}` → `https://{a}/api/groups/{g}`.
fn group_of_channel(uri: &str) -> Option<String> {
    let (group, channel) = uri.rsplit_once("/channels/")?;
    (!channel.is_empty() && !channel.contains('/') && group.contains("/api/groups/"))
        .then(|| group.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_uri_names_its_group() {
        assert_eq!(
            group_of_channel("https://a.example/api/groups/g1/channels/c1").as_deref(),
            Some("https://a.example/api/groups/g1")
        );
        assert_eq!(group_of_channel("https://a.example/api/groups/g1"), None);
        assert_eq!(group_of_channel("dm_1"), None);
    }

    #[test]
    fn a_private_group_takes_members_by_invite() {
        let group = serde_json::json!({"tier": "private", "joinPolicy": "open"});
        assert_eq!(join_policy(&group), "invite");
        let group = serde_json::json!({"tier": "public", "joinPolicy": "request"});
        assert_eq!(join_policy(&group), "request");
    }
}
