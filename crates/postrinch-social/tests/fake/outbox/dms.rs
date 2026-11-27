//! Private messages with someone on another provider: each goes to its
//! recipient's home, a reply names the provider of a parent the other
//! side holds, and edits and reactions go to the provider holding the
//! message (§7.4).

use std::sync::Arc;

use postrinch_ofscp::Endpoint;
use postrinch_ofscp::http::{Client, PageRequest};
use postrinch_ofscp::ofscp_core::wire::DmMessageCreate;
use postrinch_ofscp::ofscp_core::{DmId, dm_id};
use postrinch_ofscp_fake::FakeProvider;
use postrinch_store::SocialChange;
use serde_json::json;

use crate::common::{Home, PASSWORD, home, requests_to};

use super::{chat, kept_sent, outbox, queue, send, text};

/// Erin, on a provider of her own, signed in there, and a conversation
/// with Ada in which Erin wrote first: her message is held at Ada's home.
struct Away {
    provider: FakeProvider,
    client: Client,
    dm: DmId,
    /// Erin's message, as Ada's home holds it.
    first: String,
}

async fn erin_wrote(h: &Home) -> Away {
    let provider = FakeProvider::start().await.unwrap();
    let erin = provider.add_user("erin", PASSWORD);
    let endpoint = Endpoint::new(provider.authority());
    let identity = h
        .transport
        .provider(endpoint.clone())
        .sign_in("erin", PASSWORD, "erin's")
        .await
        .unwrap();
    let client = h.transport.client(Arc::new(identity), endpoint);
    let dm = dm_id(&erin, &h.ada);
    let hello: DmMessageCreate = serde_json::from_value(json!({
        "clientMessageId": "e1",
        "to": h.ada.to_string(),
        "content": { "text": "hi Ada", "mime": "text/plain" },
    }))
    .unwrap();
    let at_ada = h.transport.client(
        client.identity().clone(),
        Endpoint::new(h.provider.authority()),
    );
    let first = at_ada.send_dm(&dm, &hello).await.expect("to Ada");
    Away {
        provider,
        client,
        dm,
        first: first.message().id.to_string(),
    }
}

#[tokio::test]
async fn a_reply_to_a_message_held_at_home_names_its_provider() {
    let h = home().await;
    let away = erin_wrote(&h).await;
    h.add().await;
    let place = away.dm.to_string();

    // The answer goes to Erin's provider, which holds it; its parent is
    // held at Ada's, so the reference names that provider. The first
    // answer is lost: the retry is matched by its client id, after which
    // the parent is checked (§7.4).
    away.provider.faults().drop_answers("POST", "/messages", 1);
    queue(&h, "a1", &place, &chat("hi Erin", Some(&away.first)));
    assert_eq!(send(&h).await.failed, 1);
    assert_eq!(send(&h).await.sent, 1);
    assert!(outbox(&h).is_empty());

    let held = away
        .client
        .dm_messages(&away.dm, &PageRequest::newest())
        .await
        .unwrap();
    let replies: Vec<_> = held
        .items
        .iter()
        .filter_map(|item| match item {
            postrinch_ofscp::ofscp_core::wire::TimelineItem::Message(m) => Some(m),
            _ => None,
        })
        .filter(|m| m.client_message_id.as_deref() == Some("a1"))
        .collect();
    assert_eq!(replies.len(), 1, "made once");
    let reference = replies[0].reference.as_ref().expect("a reply");
    assert_eq!(reference.id.as_str(), away.first);
    assert_eq!(reference.provider.as_ref(), Some(&h.provider.authority()));

    // Kept under its URL at Erin's provider, answering Erin's message.
    let kept = kept_sent(&h, &place, "a1");
    let id = replies[0].id.as_str();
    assert_eq!(
        kept.id,
        format!(
            "https://{}/api/dms/{}/messages/{id}",
            away.provider.authority(),
            away.dm
        )
    );
    assert_eq!(kept.parent.as_deref(), Some(away.first.as_str()));
    // Home is told of the conversation, whose messages live elsewhere.
    let pointer = format!("/api/me/dms/{}", away.dm);
    assert_eq!(requests_to(&h.provider, &pointer).len(), 1);
}

#[tokio::test]
async fn edits_and_reactions_go_to_the_provider_holding_the_message() {
    let h = home().await;
    let away = erin_wrote(&h).await;
    h.add().await;
    let place = away.dm.to_string();
    queue(&h, "a1", &place, &chat("hi Erin", None));
    assert_eq!(send(&h).await.sent, 1);
    let mine = kept_sent(&h, &place, "a1").id;

    // Ada edits hers, held at Erin's provider, and reacts to Erin's, held
    // at her own; then answers her own message, which needs no provider.
    let edit = SocialChange::Edit {
        message: mine.clone(),
        content: text("hello Erin"),
    };
    let react = SocialChange::React {
        message: away.first.clone(),
        key: "👋".into(),
        on: true,
    };
    queue(&h, "e1", &place, &edit);
    queue(&h, "r1", &place, &react);
    queue(&h, "a2", &place, &chat("and again", Some(&mine)));
    let sent = send(&h).await;
    assert_eq!((sent.sent, sent.failed, sent.refused), (3, 0, 0));

    let kept = kept_sent(&h, &place, "a1");
    assert_eq!(kept.message["content"]["text"], "hello Erin");
    assert!(kept.message["editedAt"].is_string());
    let erins = h
        .messages(&place)
        .into_iter()
        .find(|m| m.id == away.first)
        .unwrap();
    assert_eq!(
        (erins.reactions[0].key.as_str(), erins.reactions[0].mine),
        ("👋", true)
    );

    let again = kept_sent(&h, &place, "a2");
    assert_eq!(again.parent.as_deref(), Some(mine.as_str()));
    let held = away
        .client
        .dm_messages(&away.dm, &PageRequest::newest())
        .await
        .unwrap();
    let answer = held
        .items
        .iter()
        .find_map(|item| match item {
            postrinch_ofscp::ofscp_core::wire::TimelineItem::Message(m)
                if m.client_message_id.as_deref() == Some("a2") =>
            {
                Some(m)
            }
            _ => None,
        })
        .expect("held at Erin's");
    assert_eq!(answer.reference.as_ref().unwrap().provider, None);
}
