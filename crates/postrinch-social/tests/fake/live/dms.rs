//! Private messages across two providers (the plan's PR 12, §7.4): a
//! conversation started here, sent both ways, merged from both
//! providers, and followed live at the other person's provider, where
//! the messages Ada sends are held.

use std::sync::Arc;
use std::time::Duration;

use postrinch_ofscp::Endpoint;
use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::wire::{DmMessageCreate, Message, MessageUpdate, ReactionPut};
use postrinch_ofscp::ofscp_core::{Actor, DmId, LocalId, dm_id};
use postrinch_ofscp_fake::FakeProvider;
use postrinch_social::{LiveEvent, MessageChange, Social, SocialError};
use postrinch_store::SocialChange;
use serde_json::json;

use super::{Told, live};
use crate::common::{Home, PASSWORD, home, requests_to};
use crate::outbox::{chat, kept_sent, queue, until_sent};

/// Erin, on a provider of her own, and the clients the tests act through.
pub struct Away {
    pub provider: FakeProvider,
    pub erin: Actor,
    /// Erin at her own provider.
    pub erin_home: Client,
    /// Erin at Ada's provider, where what she sends Ada is held.
    pub erin_at_ada: Client,
    /// Ada's phone at Erin's provider, where what Ada sends Erin is held.
    pub phone_away: Client,
    pub dm: DmId,
}

pub async fn away(h: &Home) -> Away {
    let provider = FakeProvider::start().await.unwrap();
    let erin = provider.add_user("erin", PASSWORD);
    let endpoint = Endpoint::new(provider.authority());
    let identity = h
        .transport
        .provider(endpoint.clone())
        .sign_in("erin", PASSWORD, "erin's")
        .await
        .unwrap();
    let identity = Arc::new(identity);
    let erin_home = h.transport.client(identity.clone(), endpoint.clone());
    let erin_at_ada = h
        .transport
        .client(identity, Endpoint::new(h.provider.authority()));
    let phone_away = h.transport.client(h.phone.identity().clone(), endpoint);
    let dm = dm_id(&erin, &h.ada);
    Away {
        provider,
        erin,
        erin_home,
        erin_at_ada,
        phone_away,
        dm,
    }
}

/// A message to `to` saying `text`, answering `parent` (its id and, when
/// the other side holds it, that provider) when given.
pub fn to(
    to: &Actor,
    client_id: &str,
    text: &str,
    parent: Option<(&str, Option<&str>)>,
) -> DmMessageCreate {
    let mut body = json!({
        "clientMessageId": client_id,
        "to": to.to_string(),
        "content": { "text": text, "mime": "text/plain" },
    });
    if let Some((id, provider)) = parent {
        body["reference"] = json!({ "type": "reply", "id": id });
        if let Some(provider) = provider {
            body["reference"]["provider"] = json!(provider);
        }
    }
    serde_json::from_value(body).unwrap()
}

/// The id Ada's message sent as `client_id` has where it is held: the
/// last part of the URL it is kept under.
pub fn held_id(kept: &str) -> LocalId {
    LocalId::new(kept.rsplit('/').next().unwrap()).unwrap()
}

/// Whether `event` tells of `change` to the message kept as `id`.
fn is(event: &LiveEvent, id: &str, change: MessageChange) -> bool {
    matches!(event, LiveEvent::Message { id: told, change: c, .. } if told == id && *c == change)
}

/// Whether `event` tells that the session with `provider` caught up.
pub fn caught_up_at(event: &LiveEvent, provider: &FakeProvider) -> bool {
    let authority = provider.authority().to_string();
    matches!(event, LiveEvent::CaughtUp { provider: p, .. } if *p == authority)
}

/// Wait until `provider` was asked for `path`, failing after a while.
async fn until_requested(provider: &FakeProvider, path: &str) {
    let asked = async {
        while requests_to(provider, path).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(15), asked)
        .await
        .unwrap_or_else(|_| panic!("{path} was never asked for"));
}

/// Ada live, a conversation with Erin started here, and the session at
/// Erin's provider caught up.
async fn started(h: &Home, away: &Away) -> (Social, Told, String) {
    let (social, mut told) = live(h, 1).await;
    let place = social
        .start_conversation(&h.account(), &away.erin.to_string())
        .await
        .expect("started");
    assert_eq!(place, away.dm.to_string());
    told.until("Erin's provider caught up", |e| {
        caught_up_at(e, &away.provider)
    })
    .await;
    (social, told, place)
}

#[tokio::test]
async fn a_conversation_started_here_goes_both_ways() {
    let h = home().await;
    let away = away(&h).await;
    let (social, mut told, place) = started(&h, &away).await;

    // Ada writes first: held at Erin's provider, kept under its URL there,
    // and home learns of the conversation from its pointer.
    queue(&h, "a1", &place, &chat("hi Erin", None));
    social.outbox_changed(&h.account());
    until_sent(&h).await;
    let mine = kept_sent(&h, &place, "a1");
    let at_erins = format!(
        "https://{}/api/dms/{place}/messages/",
        away.provider.authority()
    );
    assert!(mine.id.starts_with(&at_erins), "{}", mine.id);
    // The pointer is put after the message is kept, best effort.
    until_requested(&h.provider, &format!("/api/me/dms/{place}")).await;

    // Erin answers it at Ada's provider, naming the provider holding it.
    let parent = held_id(&mine.id);
    let erins = away.provider.authority().to_string();
    let answer = to(
        &h.ada,
        "e1",
        "hi Ada",
        Some((parent.as_str(), Some(&erins))),
    );
    let posted = away.erin_at_ada.send_dm(&away.dm, &answer).await.unwrap();
    let id = posted.message().id.to_string();
    told.until("Erin's answer", |e| is(e, &id, MessageChange::New))
        .await;

    let kept = h.messages(&place);
    assert_eq!(kept.len(), 2, "{kept:#?}");
    let theirs = kept
        .iter()
        .find(|m| m.id == id)
        .expect("held at home, by its id");
    assert!(theirs.unread);
    assert_eq!(theirs.parent.as_deref(), Some(mine.id.as_str()));

    // Ada answers Erin's, held at home: the reference names home.
    queue(&h, "a2", &place, &chat("how are you?", Some(&id)));
    social.outbox_changed(&h.account());
    until_sent(&h).await;
    let reply = kept_sent(&h, &place, "a2");
    assert_eq!(reply.parent.as_deref(), Some(id.as_str()));
    let held: Message = serde_json::from_value(reply.message.clone()).unwrap();
    let reference = held.reference.expect("a reply");
    assert_eq!(reference.provider, Some(h.provider.authority()));
}

#[tokio::test]
async fn reactions_edits_and_deletes_on_sent_messages_arrive_live() {
    let h = home().await;
    let away = away(&h).await;
    let (social, mut told, place) = started(&h, &away).await;
    queue(&h, "a1", &place, &chat("hi Erin", None));
    queue(&h, "a2", &place, &chat("are you there?", None));
    social.outbox_changed(&h.account());
    until_sent(&h).await;
    let first = kept_sent(&h, &place, "a1").id;
    let second = kept_sent(&h, &place, "a2").id;

    // Erin reacts at her provider, which holds Ada's message.
    let heart: ReactionPut = serde_json::from_value(json!({ "unicode": "❤️" })).unwrap();
    away.erin_home
        .put_dm_reaction(&away.dm, &held_id(&first), "heart", &heart)
        .await
        .unwrap();
    told.until(
        "Erin's reaction",
        |e| matches!(e, LiveEvent::Reactions { id, .. } if *id == first),
    )
    .await;
    let reacted = kept_sent(&h, &place, "a1");
    assert_eq!(
        (
            reacted.reactions[0].key.as_str(),
            reacted.reactions[0].count
        ),
        ("heart", 1)
    );
    assert!(!reacted.reactions[0].mine);

    // Ada edits one from her phone and deletes the other: both are held
    // at Erin's, and arrive from there.
    let update: MessageUpdate = serde_json::from_value(
        json!({ "content": { "text": "hello Erin", "mime": "text/plain" } }),
    )
    .unwrap();
    away.phone_away
        .edit_dm(&away.dm, &held_id(&first), &update)
        .await
        .unwrap();
    told.until("the edit", |e| is(e, &first, MessageChange::Edited))
        .await;
    away.phone_away
        .delete_dm(&away.dm, &held_id(&second))
        .await
        .unwrap();
    told.until("the delete", |e| is(e, &second, MessageChange::Deleted))
        .await;

    let edited = kept_sent(&h, &place, "a1");
    assert_eq!(edited.message["content"]["text"], "hello Erin");
    assert_eq!(edited.reactions.len(), 1, "an edit keeps the reactions");
    assert!(kept_sent(&h, &place, "a2").deleted);
    // The session at Erin's moved its own cursor, not home's.
    let kept = h
        .social
        .store()
        .social_place(&h.account(), &place)
        .unwrap()
        .unwrap();
    assert!(kept.cursors.sent.is_some());
    assert_eq!(h.messages(&place).len(), 2);
}

#[tokio::test]
async fn messages_from_two_providers_are_never_merged() {
    let h = home().await;
    let away = away(&h).await;
    // Each provider gives its first message of the conversation the same
    // id: they are two messages (§2.2), and each is deduplicated only
    // against its own provider's.
    away.provider.faults().duplicate_events(true);
    h.provider.faults().duplicate_events(true);
    let (social, mut told, place) = started(&h, &away).await;
    queue(&h, "a1", &place, &chat("from Ada", None));
    social.outbox_changed(&h.account());
    until_sent(&h).await;
    let mine = kept_sent(&h, &place, "a1").id;
    let posted = away
        .erin_at_ada
        .send_dm(&away.dm, &to(&h.ada, "e1", "from Erin", None))
        .await
        .unwrap();
    let theirs = posted.message().id.to_string();
    told.until("Erin's message", |e| is(e, &theirs, MessageChange::New))
        .await;
    assert_eq!(held_id(&mine).as_str(), theirs, "the fakes number alike");

    let update: MessageUpdate = serde_json::from_value(
        json!({ "content": { "text": "from Ada, edited", "mime": "text/plain" } }),
    )
    .unwrap();
    away.phone_away
        .edit_dm(&away.dm, &held_id(&mine), &update)
        .await
        .unwrap();
    told.until("the edit", |e| is(e, &mine, MessageChange::Edited))
        .await;
    told.settle(std::time::Duration::from_millis(300)).await;

    let kept = h.messages(&place);
    assert_eq!(kept.len(), 2, "{kept:#?}");
    let erins = kept.iter().find(|m| m.id == theirs).unwrap();
    assert_eq!(
        erins.message["content"]["text"], "from Erin",
        "the edit of Ada's left Erin's alone"
    );
    // Each copy of each event changed something once.
    assert_eq!(told.messages_told(&theirs), 1);
    let edits = told
        .seen
        .iter()
        .filter(|e| is(e, &mine, MessageChange::Edited))
        .count();
    assert_eq!(edits, 1);
}

/// Reading a conversation moves home's marker to the message home holds:
/// the outbox sends it, and home keeps it. Which message the commands
/// name is postrinch-commands' (its `provider_marker` tests).
#[tokio::test]
async fn a_conversations_read_marker_reaches_home() {
    let h = home().await;
    let away = away(&h).await;
    let (social, mut told, place) = started(&h, &away).await;
    let posted = away
        .erin_at_ada
        .send_dm(&away.dm, &to(&h.ada, "e1", "hi Ada", None))
        .await
        .unwrap();
    let theirs = posted.message().id.to_string();
    told.until("Erin's message", |e| is(e, &theirs, MessageChange::New))
        .await;

    let read = SocialChange::Read {
        message: theirs.clone(),
    };
    queue(&h, "r1", &place, &read);
    social.outbox_changed(&h.account());
    until_sent(&h).await;
    let markers = h.phone.read_markers().await.unwrap();
    let marker = markers
        .scopes
        .iter()
        .find(|m| m.scope_id.as_str() == place)
        .expect("home keeps the conversation's marker");
    assert_eq!(
        marker.last_read_message_id.as_ref().map(LocalId::as_str),
        Some(theirs.as_str())
    );
}

#[tokio::test]
async fn starting_needs_someone_else_who_exists() {
    let h = home().await;
    let away = away(&h).await;
    h.add().await;
    let nobody = format!("nobody@{}", away.provider.authority());
    let missing = h.social.start_conversation(&h.account(), &nobody).await;
    assert!(
        matches!(missing, Err(SocialError::NoSuchPerson(_))),
        "{missing:?}"
    );
    let alone = h
        .social
        .start_conversation(&h.account(), &format!(" {} ", h.account()))
        .await;
    assert!(
        matches!(alone, Err(SocialError::WithYourself(_))),
        "{alone:?}"
    );
    assert!(
        h.social
            .store()
            .social_places(&h.account())
            .unwrap()
            .is_empty()
    );

    // Started twice, it is kept once, as it was.
    let erin = away.erin.to_string();
    let place = h
        .social
        .start_conversation(&h.account(), &erin)
        .await
        .unwrap();
    let again = h
        .social
        .start_conversation(&h.account(), &erin)
        .await
        .unwrap();
    assert_eq!(place, again);
    assert_eq!(
        h.social.store().social_places(&h.account()).unwrap().len(),
        1
    );
}

/// Each provider holds its own half of a conversation's history (§7.4),
/// so each has its own place to scroll back from: a short history at
/// home does not make the conversation complete while what the person
/// sent goes back further at the other provider.
#[tokio::test]
async fn each_providers_history_is_gone_back_through_on_its_own() {
    let h = home().await;
    let away = away(&h).await;
    for i in 0..51 {
        let text = format!("note {i}");
        away.phone_away
            .send_dm(&away.dm, &to(&away.erin, &format!("a{i}"), &text, None))
            .await
            .unwrap();
    }
    away.erin_at_ada
        .send_dm(&away.dm, &to(&h.ada, "e1", "hi Ada", None))
        .await
        .unwrap();

    h.add().await;
    h.social.sync(&h.account()).await.unwrap();
    let kept = h
        .social
        .store()
        .social_place(&h.account(), &away.dm.to_string())
        .unwrap()
        .expect("the conversation");
    let c = &kept.cursors;
    assert!(c.complete && c.older.is_none(), "home holds one: {c:?}");
    assert!(!c.sent_complete && c.older_sent.is_some(), "{c:?}");
    assert_eq!(
        h.messages(&away.dm.to_string()).len(),
        51,
        "a page from each"
    );
}
