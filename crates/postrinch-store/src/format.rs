//! The store's format version, and opening the database.
//!
//! A store records the format it was written in. Opening one of an older
//! format that [`UPGRADES`] reaches brings it up to [`CURRENT`] in place,
//! one format at a time; each step is safe to run again, as the new version
//! is written only once the step is done. A store older than those, or one
//! from before format versions, is refused with [`StoreError::OldFormat`]
//! (the person deletes the database and adds their accounts again; raw
//! messages stay). A store written by a newer Postrinch is refused with
//! [`StoreError::NewerFormat`], and left as it is.
//!
//! The schema must be able to hold everything a store of an upgradable
//! format holds: rhypedb refuses to drop types or fields silently, and a
//! store whose types or fields the schema no longer has is refused too.

use std::path::Path;
use std::sync::Arc;

use rhypedb_engine::database::Database;
use rhypedb_engine::object::Value;
use rhypedb_engine::{CatalogError, EngineError};

use crate::codec::{Fields, FieldsBuilder};
use crate::{SCHEMA, Store, StoreError, StoreResult, blobs};

/// The format this code writes. Raise it with any change to `schema.rhype`
/// or to how the store fills it, adding the step to [`UPGRADES`] and keeping
/// the schema the last release wrote in `format_fixtures/` for the test
/// that opens such a store.
pub(crate) const CURRENT: u32 = 4;

/// A step from one format to the next: what changes, and the change.
type Upgrade = (u32, &'static str, fn(&Store) -> StoreResult<()>);

/// The steps that bring an older store up to [`CURRENT`], oldest first, each
/// from the format it names to the next. A store older than the first is
/// refused.
const UPGRADES: &[Upgrade] = &[(
    3,
    "social (OFSCP) accounts and what they hold, kept from now on",
    // The social types are new to the schema and start empty.
    |_| Ok(()),
)];

pub(crate) const INFO: &str = "StoreInfo";
pub(crate) const INFO_KEY: &str = "store";

/// Open the store under `root`, creating it if it does not exist.
pub(crate) fn open(root: &Path) -> StoreResult<Store> {
    let dir = root.join("db");
    let old = |found: String| StoreError::OldFormat {
        dir: dir.clone(),
        found,
    };
    let schema = rhypedb_schema::parser::parse_schema(SCHEMA)
        .map_err(|e| StoreError::Schema(e.to_string()))?;
    let db = match Database::open(schema, &dir) {
        Ok(db) => db,
        Err(EngineError::Catalog(CatalogError::SchemaShrinkRequiresOptIn {
            dropped_types,
            dropped_fields,
            dropped_rels,
        })) => {
            let mut dropped = dropped_types;
            dropped.extend(dropped_fields);
            dropped.extend(dropped_rels);
            return Err(old(format!(
                "it holds {dropped:?}, which this schema no longer has"
            )));
        }
        Err(e) => return Err(e.into()),
    };
    let store = Store {
        db,
        blobs: blobs::Blobs::open(root.join("blobs"))?,
        media_cache: blobs::Blobs::open(root.join("social-media"))?,
        root: root.to_path_buf(),
        outbox_lock: Arc::default(),
        outbox_cache: Arc::default(),
        thread_lock: Arc::default(),
        rules_lock: Arc::default(),
        account_lock: Arc::default(),
        cursor_lock: Arc::default(),
        social_lock: Arc::default(),
        writes: Arc::default(),
        stats: Arc::default(),
        undownloaded: Arc::default(),
        #[cfg(feature = "faults")]
        faults: Arc::default(),
    };
    let key = Value::String(INFO_KEY.into());
    match store.db.find_by_unique(INFO, "key", &key)? {
        Some(info) => {
            let version = Fields::new(INFO, info).u32("version")?;
            if version > CURRENT {
                return Err(StoreError::NewerFormat {
                    dir: dir.clone(),
                    found: version,
                    current: CURRENT,
                });
            }
            if version < CURRENT {
                upgrade(&store, version).map_err(|e| match e {
                    StoreError::OldFormat { .. } => e,
                    e => StoreError::Upgrade {
                        dir: dir.clone(),
                        from: version,
                        cause: Box::new(e),
                    },
                })?;
            }
        }
        None if store.db.count_type("Account")? > 0 => {
            return Err(old("it predates format versions".into()));
        }
        None => record_format(&store, CURRENT)?,
    }
    // Copies made before rhypedb#27 is fixed: labels and views are found
    // despite it, people are merged here.
    for (what, merged) in [
        ("labels", store.merge_duplicate_labels()?),
        ("people", store.merge_duplicate_people()?),
        ("saved views", store.merge_duplicate_views()?),
    ] {
        if merged > 0 {
            tracing::info!("merged {merged} copies of {what} into their originals");
        }
    }
    Ok(store)
}

/// Bring the store from format `from` up to [`CURRENT`], recording each
/// format as its step is done.
fn upgrade(store: &Store, from: u32) -> StoreResult<()> {
    let Some(first) = UPGRADES.iter().position(|(version, ..)| *version == from) else {
        return Err(StoreError::OldFormat {
            dir: store.root.join("db"),
            found: format!("it is format {from}, older than this Postrinch upgrades"),
        });
    };
    for (version, what, step) in &UPGRADES[first..] {
        tracing::info!("upgrading the store from format {version}: {what}");
        step(store)?;
        record_format(store, version + 1)?;
    }
    Ok(())
}

/// Record that the store is of format `version`.
fn record_format(store: &Store, version: u32) -> StoreResult<()> {
    let fields = || {
        FieldsBuilder::new()
            .str("key", INFO_KEY)
            .u32("version", version)
    };
    store.upsert(INFO, "key", INFO_KEY, fields().build(), fields().build())?;
    Ok(())
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod upgrade_tests;

#[cfg(test)]
mod tests {
    use rhypedb_schema::FieldType;

    use super::*;

    #[test]
    fn schema_keeps_message_small() {
        let schema = rhypedb_schema::parser::parse_schema(SCHEMA).unwrap();
        let message = schema.get_type("Message").unwrap();
        let to_one: Vec<&str> = message
            .fields
            .iter()
            .filter(|f| matches!(&f.field_type, FieldType::Relation(r) if !r.is_many))
            .filter(|f| f.inverse().is_none())
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            to_one,
            ["thread"],
            "rhypedb copies a message into each link's reverse entry for every other to-one relation"
        );
        for bulky in ["search_text", "body_text", "headers", "attachments"] {
            assert!(
                message.get_field(bulky).is_none(),
                "Message.{bulky} would be copied into every link and index; it belongs in MessageContent"
            );
        }
    }

    #[test]
    fn schema_keeps_imap_state_small() {
        let schema = rhypedb_schema::parser::parse_schema(SCHEMA).unwrap();
        for (type_name, relation) in [("ImapFolder", "account"), ("ImapPlace", "folder")] {
            let def = schema.get_type(type_name).unwrap();
            let to_one: Vec<&str> = def
                .fields
                .iter()
                .filter(|f| matches!(&f.field_type, FieldType::Relation(r) if !r.is_many))
                .filter(|f| f.inverse().is_none())
                .map(|f| f.name.as_str())
                .collect();
            assert_eq!(
                to_one,
                [relation],
                "a second to-one relation would copy every {type_name} into its links"
            );
        }
        // A place is copied into the index of `message`: only small fields.
        let place = schema.get_type("ImapPlace").unwrap();
        let fields: Vec<&str> = place.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            fields,
            [
                "key",
                "folder",
                "label_id",
                "uidvalidity",
                "uid",
                "message",
                "flags",
                "modseq",
                "arrived"
            ]
        );
    }

    #[test]
    fn a_new_store_records_the_current_format() {
        let dir = tempfile::tempdir().unwrap();
        drop(Store::open(dir.path()).unwrap());
        Store::open(dir.path()).unwrap();
    }

    #[test]
    fn a_newer_format_is_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        record_format(&store, CURRENT + 1).unwrap();
        drop(store);
        let err = Store::open(dir.path()).err().expect("refused");
        assert!(
            matches!(err, StoreError::NewerFormat { found, .. } if found == CURRENT + 1),
            "{err}"
        );
        // Still the newer format: nothing was written down to this one.
        let err = Store::open(dir.path()).err().expect("refused again");
        assert!(matches!(err, StoreError::NewerFormat { .. }), "{err}");
    }

    #[test]
    fn a_format_older_than_the_upgrades_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        record_format(&store, UPGRADES[0].0 - 1).unwrap();
        drop(store);
        let err = Store::open(dir.path()).err().expect("refused");
        assert!(matches!(err, StoreError::OldFormat { .. }), "{err}");
    }

    #[test]
    fn the_upgrades_reach_the_current_format_one_at_a_time() {
        let versions: Vec<u32> = UPGRADES.iter().map(|(version, ..)| *version).collect();
        let first = versions[0];
        assert_eq!(versions, (first..CURRENT).collect::<Vec<_>>());
    }

    #[test]
    fn a_schema_that_drops_data_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let extra = format!("{SCHEMA}\ntype Extra {{\n    name: String\n}}\n");
        let schema = rhypedb_schema::parser::parse_schema(&extra).unwrap();
        drop(Database::open(schema, dir.path().join("db")).unwrap());
        let err = Store::open(dir.path()).err().expect("refused");
        assert!(
            matches!(err, StoreError::OldFormat { ref found, .. } if found.contains("Extra")),
            "{err}"
        );
    }

    #[test]
    fn a_store_from_before_the_outbox_and_imap_opens() {
        let dir = tempfile::tempdir().unwrap();
        // A Windows checkout has CRLF line ends.
        let schema = SCHEMA.replace("\r\n", "\n");
        let start = schema.find("type OutboxEntry").expect("the outbox type");
        let comment = schema[..start].rfind("\n\n").expect("its comment");
        // Every type from the outbox on goes, and with them the account's
        // IMAP settings and folders.
        let before: Vec<&str> = schema[..comment]
            .lines()
            .filter(|l| !matches!(l.trim(), "imap: Json") && !l.contains("[ImapFolder]"))
            .collect();
        let schema = rhypedb_schema::parser::parse_schema(&before.join("\n")).unwrap();
        let db = Database::open(schema, dir.path().join("db")).unwrap();
        let fields = || {
            FieldsBuilder::new()
                .str("key", INFO_KEY)
                .u32("version", CURRENT)
        };
        db.create(INFO, fields().build()).unwrap();
        let account = FieldsBuilder::new()
            .str("key", "ada@example.org")
            .str("address", "ada@example.org")
            .str("provider", "google")
            .json("state", &postrinch_model::AccountState::Active)
            .build();
        db.create("Account", account).unwrap();
        drop(db);
        let store = Store::open(dir.path()).expect("an added type needs no new format");
        let accounts = store.accounts().unwrap();
        assert_eq!(accounts[0].imap, None, "an added field reads as null");
        assert!(store.imap_folders(&accounts[0].address).unwrap().is_empty());
    }
}
