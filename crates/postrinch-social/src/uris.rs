//! The canonical object URIs of groups and channels (§2.2), which the
//! store keys them by, made and read; and the URL of a private message.

use postrinch_ofscp::ofscp_core::{Authority, LocalId, ObjectRef, ObjectUri};

/// `https://{authority}/api/groups/{group}`.
pub(crate) fn group_uri(authority: &Authority, group: &LocalId) -> String {
    format!("https://{authority}/api/groups/{group}")
}

/// `https://{authority}/api/groups/{group}/channels/{channel}`.
pub(crate) fn channel_uri(authority: &Authority, group: &LocalId, channel: &LocalId) -> String {
    format!("https://{authority}/api/groups/{group}/channels/{channel}")
}

/// `https://{authority}/api/dms/{dm}/messages/{message}`: where a private
/// message is read at the provider holding it (§7.4). The spec names no
/// object URI for one; this is what the store keys a message by when it
/// is held elsewhere than the conversation's home.
pub(crate) fn dm_message_url(authority: &Authority, dm: &str, message: &str) -> String {
    format!("https://{authority}/api/dms/{dm}/messages/{message}")
}

/// The provider and id of a group's object URI; `None` for a URI that is
/// not one.
pub(crate) fn parse_group(uri: &ObjectUri) -> Option<(Authority, LocalId)> {
    let id = uri.path().strip_prefix("/api/groups/")?;
    let id = LocalId::new(id).ok()?;
    Some((uri.authority().clone(), id))
}

/// A channel named by object URI: its provider, group and id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelAt {
    pub authority: Authority,
    pub group: LocalId,
    pub channel: LocalId,
}

/// The channel a ref names, a local id being on `home` (§2.2). A local id
/// alone does not say which group the channel is in, so it needs the
/// follow's `groupId`; `None` when it cannot be placed.
pub(crate) fn parse_channel(
    channel: &ObjectRef,
    group: Option<&ObjectRef>,
    home: &Authority,
) -> Option<ChannelAt> {
    match channel {
        ObjectRef::Uri(uri) => {
            let rest = uri.path().strip_prefix("/api/groups/")?;
            let (group, channel) = rest.split_once("/channels/")?;
            Some(ChannelAt {
                authority: uri.authority().clone(),
                group: LocalId::new(group).ok()?,
                channel: LocalId::new(channel).ok()?,
            })
        }
        ObjectRef::Local(channel) => {
            let group = match group? {
                ObjectRef::Local(id) => id.clone(),
                ObjectRef::Uri(uri) => {
                    let (authority, id) = parse_group(uri)?;
                    if &authority != home {
                        return None;
                    }
                    id
                }
            };
            Some(ChannelAt {
                authority: home.clone(),
                group,
                channel: channel.clone(),
            })
        }
    }
}

/// The provider and id of a group's URI as the store keeps it.
pub(crate) fn parse_group_uri(uri: &str) -> Option<(Authority, LocalId)> {
    parse_group(&ObjectUri::parse(uri).ok()?)
}

/// The channel a channel's URI, as the store keeps it, names.
pub(crate) fn parse_channel_uri(uri: &str) -> Option<ChannelAt> {
    let uri = ObjectUri::parse(uri).ok()?;
    parse_channel(&ObjectRef::Uri(uri.clone()), None, uri.authority())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(s: &str) -> Authority {
        Authority::canonicalize(s).unwrap()
    }

    fn id(s: &str) -> LocalId {
        LocalId::new(s).unwrap()
    }

    #[test]
    fn uris_are_made_and_read_back() {
        let a = authority("a.example");
        let group = group_uri(&a, &id("grp_1"));
        assert_eq!(group, "https://a.example/api/groups/grp_1");
        let parsed = parse_group(&ObjectUri::parse(&group).unwrap()).unwrap();
        assert_eq!(parsed, (a.clone(), id("grp_1")));

        let channel = channel_uri(&a, &id("grp_1"), &id("chn_2"));
        let parsed = parse_channel(&ObjectRef::parse(&channel).unwrap(), None, &a).unwrap();
        assert_eq!(parsed.channel, id("chn_2"));
        assert_eq!(parsed.group, id("grp_1"));

        assert_eq!(parse_group_uri(&group), Some((a.clone(), id("grp_1"))));
        let at = parse_channel_uri(&channel).unwrap();
        assert_eq!((at.authority, at.channel), (a, id("chn_2")));
        assert_eq!(parse_channel_uri(&group), None);
        assert_eq!(parse_channel_uri("not a uri"), None);
    }

    #[test]
    fn a_local_channel_needs_its_group() {
        let home = authority("a.example");
        let channel = ObjectRef::parse("chn_2").unwrap();
        assert_eq!(parse_channel(&channel, None, &home), None);
        let group = ObjectRef::parse("grp_1").unwrap();
        let at = parse_channel(&channel, Some(&group), &home).unwrap();
        assert_eq!((at.authority, at.group), (home.clone(), id("grp_1")));
        let elsewhere = ObjectRef::parse("https://b.example/api/groups/grp_1").unwrap();
        assert_eq!(parse_channel(&channel, Some(&elsewhere), &home), None);
    }

    #[test]
    fn other_uris_are_not_groups_or_channels() {
        let user = ObjectUri::parse("https://a.example/api/users/ada").unwrap();
        assert_eq!(parse_group(&user), None);
        let nested = ObjectUri::parse("https://a.example/api/groups/g/channels/c").unwrap();
        assert_eq!(parse_group(&nested), None);
        let channel = ObjectRef::Uri(user);
        assert_eq!(parse_channel(&channel, None, &authority("a.example")), None);
    }
}
