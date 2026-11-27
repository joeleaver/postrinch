//! Mentions and the notification feed (§10.1).

use ofscp_core::wire::{Message, Notification, NotificationKind};
use ofscp_core::{Actor, Authority, LocalId};

use crate::state::State;

/// The actors a message mentions: its `mentions` when it has them, else
/// the `@handle` and `@handle@domain` in its text (§10.1 *Mentions*).
pub(crate) fn mentioned(message: &Message, host: &Authority) -> Vec<Actor> {
    if let Some(m) = &message.mentions {
        return m.clone();
    }
    let text = message.content.as_ref().map_or("", |c| c.text.as_str());
    parse_mentions(text, host)
}

fn handle_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn parse_mentions(text: &str, host: &Authority) -> Vec<Actor> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let prev_ok = i == 0
            || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == '@');
        if chars[i] != '@' || !prev_ok {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < chars.len() && handle_char(chars[end]) {
            end += 1;
        }
        if end == start {
            i += 1;
            continue;
        }
        let handle: String = chars[start..end].iter().collect::<String>().to_lowercase();
        let mut domain = host.clone();
        let mut next = end;
        if end < chars.len() && chars[end] == '@' {
            let mut d = end + 1;
            while d < chars.len()
                && (chars[d].is_ascii_alphanumeric() || matches!(chars[d], '.' | '-' | ':'))
            {
                d += 1;
            }
            let text: String = chars[end + 1..d].iter().collect();
            if let Ok(a) = Authority::canonicalize(text.trim_end_matches('.')) {
                domain = a;
                next = d;
            }
        }
        if let Ok(actor) = Actor::new(&handle, domain)
            && !found.contains(&actor)
        {
            found.push(actor);
        }
        i = next;
    }
    found
}

/// Create the feed entries a new channel message makes (§10.1 *When
/// entries are created*), with the default mode `mentions` everywhere.
/// Returns each entry and whose it is.
pub(crate) fn on_create(
    state: &mut State,
    authority: &Authority,
    group: &LocalId,
    channel: &LocalId,
    message: &Message,
) -> Vec<(Actor, Notification)> {
    let mentioned = mentioned(message, authority);
    let parent_author = message.reference.as_ref().and_then(|r| {
        state
            .channels
            .get(channel)?
            .timeline
            .get(&r.id)
            .map(|s| s.message.author.clone())
    });
    let (Some(g), Some(c)) = (state.groups.get(group), state.channels.get(channel)) else {
        return Vec::new();
    };
    let readers: Vec<(Actor, NotificationKind)> = state
        .users
        .values()
        .map(|u| &u.actor)
        .filter(|u| **u != message.author && g.can_read(&c.channel, Some(u)))
        .filter_map(|u| {
            let kind = if mentioned.contains(u) {
                NotificationKind::Mention
            } else if parent_author.as_ref() == Some(u) {
                NotificationKind::Reply
            } else {
                return None;
            };
            Some((u.clone(), kind))
        })
        .collect();
    let mut made = Vec::new();
    for (actor, kind) in readers {
        let notification = Notification {
            id: state.ids.next("ntf_"),
            kind,
            source_message_id: message.id.clone(),
            channel_id: channel.clone(),
            group_id: group.clone(),
            author: message.author.clone(),
            created_at: message.created_at,
            seen_at: None,
            read_at: None,
        };
        if let Some(user) = state.users.get_mut(actor.handle()) {
            user.notifications.push(notification.clone());
        }
        made.push((actor, notification));
    }
    made
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Authority {
        Authority::canonicalize("a.test").unwrap()
    }

    fn names(text: &str) -> Vec<String> {
        parse_mentions(text, &host())
            .iter()
            .map(|a| a.to_string())
            .collect()
    }

    #[test]
    fn mentions_follow_the_rules_of_10_1() {
        assert_eq!(
            names("hi @Bob and @carol@b.test!"),
            ["bob@a.test", "carol@b.test"]
        );
        assert!(names("mail bob@example.com").is_empty());
        assert_eq!(names("@@x @bob @bob"), ["bob@a.test"]);
        assert_eq!(names("(@dave-1_x)"), ["dave-1_x@a.test"]);
    }
}
