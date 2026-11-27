//! A provider's message as the store keeps it: its scalars, its reactions
//! summed, the text search finds it by, and whether it mentions the
//! person.

use std::collections::HashMap;

use postrinch_ofscp::ofscp_core::wire::{
    DmConversation, Message, Reaction, ReferenceKind, TimelineItem,
};
use postrinch_ofscp::ofscp_core::{Actor, Authority};
use postrinch_render::social::{Attachment, Body, Options, render};
use postrinch_store::{ReactionSum, SocialMessage, social_account_key};

use crate::uris::dm_message_url;

/// Who reads a place, and where it is: what a message is kept against.
pub(crate) struct Reader<'a> {
    /// The account's actor.
    pub account: &'a Actor,
    /// The place's id (a channel URI, or a dmId).
    pub place: &'a str,
    /// The provider hosting the place: home, for a conversation.
    pub host: &'a Authority,
}

/// A message to keep, with the text search finds it by.
pub(crate) struct Kept {
    pub message: SocialMessage,
    pub search_text: String,
}

/// A message as one provider holds it, with the reactions it gave for it.
pub(crate) struct Held {
    /// The provider holding it: the place's host, except in a
    /// conversation, where each message lives with its recipient (§7.4).
    pub holder: Authority,
    pub message: Message,
    pub reactions: Vec<Reaction>,
}

/// One provider's page of messages, each with the reactions the page
/// carries for it (§7.2: reactions are items of the page too), oldest
/// first.
pub(crate) fn held_by(holder: &Authority, items: Vec<TimelineItem>) -> Vec<Held> {
    let mut messages = Vec::new();
    let mut reactions: HashMap<String, Vec<Reaction>> = HashMap::new();
    for item in items {
        match item {
            TimelineItem::Message(message) => messages.push(*message),
            // A reaction is kept with its message, by the provider holding
            // it (§7.1 *Reactions*): one naming another provider is on no
            // message of this page.
            TimelineItem::Reaction(reaction)
                if reaction
                    .reference
                    .provider
                    .as_ref()
                    .is_none_or(|p| p == holder) =>
            {
                reactions
                    .entry(reaction.reference.id.to_string())
                    .or_default()
                    .push(*reaction);
            }
            TimelineItem::Reaction(_) => {}
        }
    }
    let mut held: Vec<Held> = messages
        .into_iter()
        .map(|message| Held {
            holder: holder.clone(),
            reactions: reactions.remove(message.id.as_str()).unwrap_or_default(),
            message,
        })
        .collect();
    in_order(&mut held);
    held
}

/// Put messages, perhaps from two providers, oldest first (§7.4: a
/// conversation is merged by `createdAt`). Their ids cannot repeat: each
/// provider holds different messages, and one id from two providers
/// names two of them (§2.2).
pub(crate) fn in_order(held: &mut [Held]) {
    held.sort_by(|a, b| {
        a.message
            .created_at
            .cmp(&b.message.created_at)
            .then_with(|| a.holder.cmp(&b.holder))
            .then_with(|| a.message.id.as_str().cmp(b.message.id.as_str()))
    });
}

/// The id `holder`'s message `id` is kept under in `reader`'s place. Ids
/// are unique only at the provider that gave them (§2.2): a message the
/// place's host holds keeps its own, and one of a conversation held at the
/// other person's provider is kept under its URL there.
pub(crate) fn kept_id(reader: &Reader, holder: &Authority, id: &str) -> String {
    if holder == reader.host {
        id.to_owned()
    } else {
        dm_message_url(holder, reader.place, id)
    }
}

/// The provider holding a private message of `conversation` written by
/// `author`: its recipient's home, the participant who is not its author
/// (§7.4). It is derived, not taken from whichever provider served the
/// copy. `None` when `author` is not a participant.
pub(crate) fn dm_holder(conversation: &DmConversation, author: &Actor) -> Option<Authority> {
    let [a, b] = &conversation.participants;
    let recipient = if a == author {
        b
    } else if b == author {
        a
    } else {
        return None;
    };
    Some(recipient.domain().clone())
}

/// `held` as `reader` keeps it, `unread` if it is new here.
pub(crate) fn keep(reader: &Reader, held: &Held, unread: bool) -> Kept {
    let Held {
        holder,
        message,
        reactions,
    } = held;
    let me = social_account_key(&reader.account.to_string());
    // A bare `@handle` names a user of the provider holding the message.
    let (mentioned, text) = read_body(holder, message);
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
        // A parent is named by (provider, id): the reference's provider
        // when it names one, else the provider holding this reply (§5.3).
        .map(|r| kept_id(reader, r.provider.as_ref().unwrap_or(holder), r.id.as_str()));
    let kept = SocialMessage {
        account: reader.account.to_string(),
        place: reader.place.to_owned(),
        id: kept_id(reader, holder, message.id.as_str()),
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
        let held = Held {
            holder: host.clone(),
            message: m,
            reactions: vec![reaction("ada"), reaction("carl")],
        };
        let kept = keep(&reader, &held, true);
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
        let held = Held {
            holder: host.clone(),
            message: m,
            reactions: Vec::new(),
        };
        let kept = keep(&reader, &held, false);
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
        let (_, host) = reader_parts();
        let page = held_by(&host, items);
        let ids: Vec<&str> = page.iter().map(|h| h.message.id.as_str()).collect();
        assert_eq!(ids, ["msg_1", "msg_2"]);
        assert_eq!(page[0].reactions.len(), 1);
        assert!(page[1].reactions.is_empty());
    }

    #[test]
    fn one_id_from_both_sides_of_a_conversation_is_two_messages() {
        let (ada, home) = reader_parts();
        let away = Authority::canonicalize("b.example").unwrap();
        let reader = Reader {
            account: &ada,
            place: "dm_1",
            host: &home,
        };
        let m = |author: &str, minute: u8| -> Message {
            message(json!({
                "id": "msg_1", "author": author, "type": "message",
                "content": { "text": "x", "mime": "text/plain" }, "attachments": [],
                "reference": { "type": "reply", "id": "msg_0" },
                "createdAt": format!("2026-01-01T12:0{minute}:00Z"),
            }))
        };
        let mut held = vec![
            Held {
                holder: away.clone(),
                message: m("ada@a.example", 2),
                reactions: Vec::new(),
            },
            Held {
                holder: home.clone(),
                message: m("erin@b.example", 1),
                reactions: Vec::new(),
            },
        ];
        in_order(&mut held);
        let kept: Vec<_> = held
            .iter()
            .map(|h| keep(&reader, h, false).message)
            .collect();
        assert_eq!(kept[0].id, "msg_1");
        assert_eq!(kept[0].parent.as_deref(), Some("msg_0"));
        let there = "https://b.example/api/dms/dm_1/messages/";
        assert_eq!(kept[1].id, format!("{there}msg_1"));
        assert_eq!(kept[1].parent, Some(format!("{there}msg_0")));
    }

    #[test]
    fn a_reply_names_a_parent_on_the_other_side_by_its_provider() {
        let (ada, home) = reader_parts();
        let away = Authority::canonicalize("b.example").unwrap();
        let reader = Reader {
            account: &ada,
            place: "dm_1",
            host: &home,
        };
        let reply = |provider: Option<&str>| -> Kept {
            let mut reference = json!({ "type": "reply", "id": "msg_0" });
            if let Some(p) = provider {
                reference["provider"] = json!(p);
            }
            let held = Held {
                holder: away.clone(),
                message: message(json!({
                    "id": "msg_5", "author": "ada@a.example", "type": "message",
                    "content": { "text": "x", "mime": "text/plain" }, "attachments": [],
                    "reference": reference, "createdAt": "2026-01-01T12:00:00Z",
                })),
                reactions: Vec::new(),
            };
            keep(&reader, &held, false)
        };
        let there = "https://b.example/api/dms/dm_1/messages/msg_0";
        // Ada's reply, held at Erin's provider, to a message Erin sent
        // her, held at Ada's home: its parent keeps its bare id.
        assert_eq!(
            reply(Some("a.example")).message.parent.as_deref(),
            Some("msg_0")
        );
        // Absent, or naming the holder itself, is the holder's own message.
        assert_eq!(reply(None).message.parent.as_deref(), Some(there));
        assert_eq!(
            reply(Some("b.example")).message.parent.as_deref(),
            Some(there)
        );
    }

    #[test]
    fn a_reaction_naming_another_provider_is_on_no_message_of_the_page() {
        let items: Vec<TimelineItem> = serde_json::from_value(json!([
            { "id": "msg_1", "author": "bob@a.example", "type": "message",
              "content": { "text": "one", "mime": "text/plain" }, "attachments": [],
              "createdAt": "2026-01-01T12:01:00Z" },
            { "id": "r_1", "author": "bob@a.example", "key": "+1",
              "reference": { "type": "message", "id": "msg_1", "provider": "b.example" },
              "createdAt": "2026-01-01T12:03:00Z" },
            { "id": "r_2", "author": "bob@a.example", "key": "ok",
              "reference": { "type": "message", "id": "msg_1", "provider": "a.example" },
              "createdAt": "2026-01-01T12:04:00Z" },
        ]))
        .unwrap();
        let (_, host) = reader_parts();
        let page = held_by(&host, items);
        let keys: Vec<&str> = page[0].reactions.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["ok"]);
    }

    #[test]
    fn a_private_message_is_held_by_its_recipients_home() {
        let actor = |a: &str| Actor::parse_canonical(a).unwrap();
        let (ada, erin) = (actor("ada@a.example"), actor("erin@b.example"));
        let conversation = DmConversation {
            id: postrinch_ofscp::ofscp_core::dm_id(&ada, &erin),
            participants: [ada, erin],
            last_message: None,
            updated_at: "2026-01-01T12:00:00Z".parse().unwrap(),
            metadata: Vec::new(),
        };
        let held = |author: &str| dm_holder(&conversation, &actor(author)).map(|h| h.to_string());
        assert_eq!(held("ada@a.example").as_deref(), Some("b.example"));
        assert_eq!(held("erin@b.example").as_deref(), Some("a.example"));
        assert_eq!(held("bob@a.example"), None);
    }
}
