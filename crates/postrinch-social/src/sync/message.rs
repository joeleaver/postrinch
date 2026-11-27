//! A provider's message as the store keeps it: its scalars, its reactions
//! summed, the text search finds it by, and whether it mentions the
//! person.

use std::collections::HashMap;

use postrinch_ofscp::ofscp_core::wire::{Message, Reaction, ReferenceKind, TimelineItem};
use postrinch_ofscp::ofscp_core::{Actor, Authority};
use postrinch_render::social::{Attachment, Body, Options, render};
use postrinch_store::{ReactionSum, SocialMessage, social_account_key};

/// Who reads a place, and where it is: what a message is kept against.
pub(crate) struct Reader<'a> {
    /// The account's actor.
    pub account: &'a Actor,
    /// The place's id (a channel URI, or a dmId).
    pub place: &'a str,
    /// The provider hosting the place: whom a bare `@handle` names.
    pub host: &'a Authority,
}

/// A message to keep, with the text search finds it by.
pub(crate) struct Kept {
    pub message: SocialMessage,
    pub search_text: String,
}

/// A page's messages, oldest first, each with the reactions the page
/// carries for it summed (§7.2: reactions are items of the page too).
pub(crate) fn messages_of(items: Vec<TimelineItem>) -> Vec<(Message, Vec<Reaction>)> {
    let mut messages = Vec::new();
    let mut reactions: HashMap<String, Vec<Reaction>> = HashMap::new();
    for item in items {
        match item {
            TimelineItem::Message(message) => messages.push(*message),
            TimelineItem::Reaction(reaction) => reactions
                .entry(reaction.reference.id.to_string())
                .or_default()
                .push(*reaction),
        }
    }
    messages.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });
    messages
        .into_iter()
        .map(|m| {
            let on = reactions.remove(m.id.as_str()).unwrap_or_default();
            (m, on)
        })
        .collect()
}

/// `message` as `reader` keeps it, `unread` if it is new here.
pub(crate) fn keep(
    reader: &Reader,
    message: &Message,
    reactions: &[Reaction],
    unread: bool,
) -> Kept {
    let me = social_account_key(&reader.account.to_string());
    let (mentioned, text) = read_body(reader.host, message);
    let author = message.author.to_string();
    let mut search_text = author.clone();
    for part in [message.title.as_deref(), message.summary.as_deref()]
        .into_iter()
        .flatten()
    {
        search_text.push('\n');
        search_text.push_str(part);
    }
    if let Some(tags) = &message.tags {
        search_text.push('\n');
        search_text.push_str(&tags.join(" "));
    }
    if !text.is_empty() {
        search_text.push('\n');
        search_text.push_str(&text);
    }
    let kind = serde_json::to_value(&message.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let parent = message
        .reference
        .as_ref()
        .filter(|r| r.kind == ReferenceKind::Reply)
        .map(|r| r.id.to_string());
    let kept = SocialMessage {
        account: reader.account.to_string(),
        place: reader.place.to_owned(),
        id: message.id.to_string(),
        author,
        kind,
        created: message.created_at.as_millisecond(),
        parent,
        deleted: message.deleted_at.is_some(),
        mentions_me: mentioned.contains(&me),
        unread,
        message: serde_json::to_value(message).expect("a wire message always serializes"),
        reactions: sum(reactions, &me),
    };
    Kept {
        message: kept,
        search_text,
    }
}

/// The people a message's body marks as mentioned (lower-cased actors),
/// by the spec's mention syntax as rendering reads it, and its text as
/// its reader sees it. A tombstone has neither.
fn read_body(host: &Authority, message: &Message) -> (Vec<String>, String) {
    let Some(content) = &message.content else {
        return (Vec::new(), String::new());
    };
    let attachments: Vec<Attachment> = message
        .attachments
        .iter()
        .map(|a| Attachment {
            id: a.id.as_str(),
            mime: &a.mime,
            name: a.filename.as_deref(),
            picture: None,
        })
        .collect();
    let listed: Option<Vec<String>> = message
        .mentions
        .as_ref()
        .map(|m| m.iter().map(ToString::to_string).collect());
    let domain = host.to_string();
    let body = Body {
        mime: &content.mime,
        text: &content.text,
        attachments: &attachments,
        mentions: listed.as_deref(),
        domain: &domain,
    };
    let rendered = render(&body, Options::default());
    let text = rinch_html::text(&rendered.html).visible;
    (rendered.mentions, text)
}

/// Reactions summed by key, in the order each key was first given.
fn sum(reactions: &[Reaction], me: &str) -> Vec<ReactionSum> {
    let mut sums: Vec<ReactionSum> = Vec::new();
    for reaction in reactions {
        let mine = social_account_key(&reaction.author.to_string()) == me;
        match sums.iter_mut().find(|s| s.key == reaction.key) {
            Some(sum) => {
                sum.count += 1;
                sum.mine |= mine;
            }
            None => sums.push(ReactionSum {
                key: reaction.key.clone(),
                count: 1,
                mine,
            }),
        }
    }
    sums
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }

    fn reader_parts() -> (Actor, Authority) {
        let host = Authority::canonicalize("a.example").unwrap();
        (Actor::new("ada", host.clone()).unwrap(), host)
    }

    #[test]
    fn a_message_is_kept_with_its_text_mentions_and_reactions() {
        let (ada, host) = reader_parts();
        let reader = Reader {
            account: &ada,
            place: "https://a.example/api/groups/g/channels/c",
            host: &host,
        };
        let m = message(json!({
            "id": "msg_2", "author": "bob@a.example", "type": "message",
            "content": { "text": "Hi @ada, see <b>this</b>", "mime": "text/html" },
            "attachments": [],
            "reference": { "type": "reply", "id": "msg_1" },
            "createdAt": "2026-01-01T12:00:00Z",
        }));
        let reaction = |author: &str| -> Reaction {
            serde_json::from_value(json!({
                "id": format!("r_{author}"), "author": format!("{author}@a.example"),
                "key": "heart", "reference": { "type": "message", "id": "msg_2" },
                "createdAt": "2026-01-01T12:01:00Z",
            }))
            .unwrap()
        };
        let kept = keep(&reader, &m, &[reaction("ada"), reaction("carl")], true);
        let k = &kept.message;
        assert_eq!(k.parent.as_deref(), Some("msg_1"));
        assert_eq!(k.kind, "message");
        assert!(k.mentions_me);
        assert!(k.unread);
        assert_eq!(k.created, 1_767_268_800_000);
        assert_eq!(
            k.reactions,
            vec![ReactionSum {
                key: "heart".into(),
                count: 2,
                mine: true
            }]
        );
        assert!(kept.search_text.starts_with("bob@a.example"));
        assert!(
            kept.search_text.contains("see this"),
            "{}",
            kept.search_text
        );
        assert!(!kept.search_text.contains("<b>"));
    }

    #[test]
    fn a_tombstone_mentions_no_one() {
        let (ada, host) = reader_parts();
        let reader = Reader {
            account: &ada,
            place: "p",
            host: &host,
        };
        let m = message(json!({
            "id": "msg_9", "author": "bob@a.example", "type": "message",
            "attachments": [], "createdAt": "2026-01-01T12:00:00Z",
            "deletedAt": "2026-01-01T12:05:00Z",
        }));
        let kept = keep(&reader, &m, &[], false);
        assert!(kept.message.deleted);
        assert!(!kept.message.mentions_me);
        assert_eq!(kept.search_text, "bob@a.example");
    }

    #[test]
    fn a_page_is_put_in_order_with_its_reactions() {
        let items: Vec<TimelineItem> = serde_json::from_value(json!([
            { "id": "msg_2", "author": "bob@a.example", "type": "message",
              "content": { "text": "two", "mime": "text/plain" }, "attachments": [],
              "createdAt": "2026-01-01T12:02:00Z" },
            { "id": "r_1", "author": "bob@a.example", "key": "+1",
              "reference": { "type": "message", "id": "msg_1" },
              "createdAt": "2026-01-01T12:03:00Z" },
            { "id": "msg_1", "author": "bob@a.example", "type": "message",
              "content": { "text": "one", "mime": "text/plain" }, "attachments": [],
              "createdAt": "2026-01-01T12:01:00Z" },
        ]))
        .unwrap();
        let page = messages_of(items);
        let ids: Vec<&str> = page.iter().map(|(m, _)| m.id.as_str()).collect();
        assert_eq!(ids, ["msg_1", "msg_2"]);
        assert_eq!(page[0].1.len(), 1);
        assert!(page[1].1.is_empty());
    }
}
