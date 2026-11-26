//! Opening a store written by the last release, in its format.

use rhypedb_engine::database::Database;

use super::{CURRENT, INFO, INFO_KEY};
use crate::codec::{Fields, FieldsBuilder};
use crate::{ProfileChange, SocialAccount, SocialAccountState, SocialChange, Store};

/// The schema the last release (Postrinch 0.1.38, format 3) wrote. Kept as
/// it was: a store it wrote is what an upgrade starts from.
const RELEASED_SCHEMA: &str = include_str!("format_fixtures/schema-format-3.rhype");
const RELEASED_FORMAT: u32 = 3;

/// A store as the last release leaves it: its schema, its format, and an
/// account with a label, a thread and a message in it.
fn released_store(root: &std::path::Path) {
    // A Windows checkout has CRLF line ends.
    let schema = RELEASED_SCHEMA.replace("\r\n", "\n");
    let schema = rhypedb_schema::parser::parse_schema(&schema).unwrap();
    let db = Database::open(schema, root.join("db")).unwrap();
    let info = FieldsBuilder::new()
        .str("key", INFO_KEY)
        .u32("version", RELEASED_FORMAT)
        .build();
    db.create(INFO, info).unwrap();
    let account = FieldsBuilder::new()
        .str("key", "ada@example.org")
        .str("address", "ada@example.org")
        .str("provider", "google")
        .json("state", &postrinch_model::AccountState::Active)
        .build();
    let account = db.create("Account", account).unwrap().id;
    let label = FieldsBuilder::new()
        .str("key", "ada@example.org\u{1f}INBOX")
        .relation("account", account)
        .str("provider_id", "INBOX")
        .str("name", "INBOX")
        .json(
            "kind",
            &postrinch_model::LabelKind::System(postrinch_model::SystemLabel::Inbox),
        )
        .build();
    db.create("Label", label).unwrap();
    let thread = FieldsBuilder::new()
        .str("key", "ada@example.org\u{1f}t1")
        .relation("account", account)
        .str("provider_id", "t1")
        .build();
    let thread = db.create("Thread", thread).unwrap().id;
    let message = FieldsBuilder::new()
        .str("key", "ada@example.org\u{1f}m1")
        .relation("thread", thread)
        .str("provider_id", "m1")
        .str("subject", "Hello")
        .str("snippet", "Hello there")
        .datetime("received", 1_700_000_000_000)
        .bool("unread", true)
        .bool("starred", false)
        .bool("has_attachments", false)
        .u64("size", 120)
        .bool("downloaded", false)
        .build();
    db.create("Message", message).unwrap();
    drop(db);
}

fn recorded_format(store: &Store) -> u32 {
    let info = store.find(INFO, "key", INFO_KEY).unwrap().unwrap();
    Fields::u32(&info, "version").unwrap()
}

#[test]
fn a_store_of_the_last_release_is_upgraded_and_keeps_its_mail() {
    let dir = tempfile::tempdir().unwrap();
    released_store(dir.path());

    let store = Store::open(dir.path()).expect("the last release's store opens");
    assert_eq!(recorded_format(&store), CURRENT);
    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    let ada = &accounts[0].address;
    assert_eq!(store.message_ids(ada).unwrap().len(), 1);
    assert!(store.social_accounts().unwrap().is_empty());

    // What the new format holds is written and kept.
    let account = SocialAccount {
        actor: "ada@groups.example".into(),
        state: SocialAccountState::Active,
        profile: None,
        settings: None,
        key_id: Some("dk_1".into()),
    };
    store.put_social_account(&account).unwrap();
    // The last release's info has no outbox counter: the first change
    // takes the first number.
    let change = &SocialChange::Profile(ProfileChange::default());
    let queued = store
        .queue_social_change(&account.actor, "c1", None, change, 1)
        .unwrap();
    assert_eq!(queued.seq, 1);
    drop(store);
    let store = Store::open(dir.path()).expect("the upgraded store opens again");
    assert_eq!(recorded_format(&store), CURRENT);
    assert_eq!(store.social_accounts().unwrap(), [account]);
    assert_eq!(store.accounts().unwrap().len(), 1);
}

#[test]
fn the_released_schema_is_part_of_this_one() {
    // An upgrade never has to drop what the last release kept: every type
    // and field of its schema is still here, of the same type.
    let released = RELEASED_SCHEMA.replace("\r\n", "\n");
    let released = rhypedb_schema::parser::parse_schema(&released).unwrap();
    let current = rhypedb_schema::parser::parse_schema(crate::SCHEMA).unwrap();
    for old in released.types.values() {
        let new = current
            .get_type(&old.name)
            .unwrap_or_else(|| panic!("{} is gone", old.name));
        for field in &old.fields {
            let kept = new
                .get_field(&field.name)
                .unwrap_or_else(|| panic!("{}.{} is gone", old.name, field.name));
            assert_eq!(
                format!("{:?}", kept.field_type),
                format!("{:?}", field.field_type),
                "{}.{} changed type",
                old.name,
                field.name
            );
        }
    }
}
