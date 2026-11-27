//! Tests of the social store, with the records they share.

mod accounts;
mod messages;
mod outbox;
mod places;
mod writing;

use serde_json::json;

use super::*;
use crate::Store;

const ADA: &str = "ada@groups.example";
const BOB: &str = "bob@other.example";
const GENERAL: &str = "https://groups.example/api/groups/g1/channels/general";
const GROUP: &str = "https://groups.example/api/groups/g1";

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn account(actor: &str) -> SocialAccount {
    SocialAccount {
        actor: actor.into(),
        state: SocialAccountState::Active,
        profile: Some(json!({ "handle": "ada", "displayName": "Ada" })),
        settings: None,
        key_id: Some("dk_1".into()),
    }
}

fn group(account: &str, uri: &str) -> SocialGroup {
    SocialGroup {
        account: account.into(),
        uri: uri.into(),
        provider: "groups.example".into(),
        group: json!({ "id": uri, "name": "Group" }),
        role: Some("member".into()),
        found: GroupFound::Member,
        notices: None,
    }
}

fn channel(account: &str, id: &str, last_activity: Option<i64>) -> SocialPlace {
    SocialPlace {
        account: account.into(),
        id: id.into(),
        kind: PlaceKind::Channel,
        group: Some(GROUP.into()),
        followed: true,
        last_activity,
        object: json!({ "id": id, "name": "general" }),
        cursors: PlaceCursors::default(),
        read_marker: None,
        notices: Some(Notices::MentionsAndReplies),
    }
}

fn message(account: &str, place: &str, id: &str, created: i64) -> SocialMessage {
    SocialMessage {
        account: account.into(),
        place: place.into(),
        id: id.into(),
        author: "bob@other.example".into(),
        kind: "message".into(),
        created,
        parent: None,
        deleted: false,
        mentions_me: false,
        unread: true,
        message: json!({ "id": id, "content": { "text": format!("message {id}") } }),
        reactions: vec![],
    }
}

/// A store with Ada's account, her group and its channel.
fn ada_with_general() -> (tempfile::TempDir, Store) {
    let (dir, store) = store();
    store.put_social_account(&account(ADA)).unwrap();
    store.put_social_group(&group(ADA, GROUP)).unwrap();
    store
        .put_social_place(&channel(ADA, GENERAL, None))
        .unwrap();
    (dir, store)
}

#[test]
fn schema_keeps_social_message_small() {
    use rhypedb_schema::FieldType;
    let schema = rhypedb_schema::parser::parse_schema(crate::SCHEMA).unwrap();
    let message = schema.get_type("SocialMessage").unwrap();
    let to_one: Vec<&str> = message
        .fields
        .iter()
        .filter(|f| matches!(&f.field_type, FieldType::Relation(r) if !r.is_many))
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        to_one.is_empty(),
        "rhypedb copies a message into the reverse entry of every link of a type with a to-one relation: {to_one:?}"
    );
    // A message is copied into each of its indexes: only small scalars.
    let fields: Vec<&str> = message.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        fields,
        [
            "key",
            "place",
            "author",
            "kind",
            "created",
            "parent",
            "deleted",
            "mentions_me",
            "unread",
            "content"
        ],
        "SocialMessage holds small scalars only; what a message says belongs in SocialMessageContent"
    );
    let content = schema.get_type("SocialMessageContent").unwrap();
    let to_one = content
        .fields
        .iter()
        .filter(|f| matches!(&f.field_type, FieldType::Relation(r) if !r.is_many))
        .count();
    assert_eq!(to_one, 1, "a second to-one relation would copy the content");
    assert!(
        content.fields.iter().all(|f| !f.is_indexed()),
        "an @indexed field would copy the content into its index"
    );
}

/// A change moving the read marker to `message`.
fn read_change(message: &str) -> SocialChange {
    SocialChange::Read {
        message: message.into(),
    }
}
