//! The store's format version, and opening the database.
//!
//! Postrinch is in development: when the store's format changes, an older
//! store is not upgraded. Opening one fails with [`StoreError::OldFormat`],
//! and the person deletes the database (raw messages stay) and adds their
//! accounts again. Upgrades in place come with the first release.
//!
//! A store is of another format when its recorded version differs from
//! [`CURRENT`], or when it predates versions, or when the schema no longer
//! has types or fields it holds (rhypedb refuses to drop them silently).

use std::path::Path;
use std::sync::Arc;

use rhypedb_engine::database::Database;
use rhypedb_engine::object::Value;
use rhypedb_engine::{CatalogError, EngineError};

use crate::codec::{Fields, FieldsBuilder};
use crate::{SCHEMA, Store, StoreError, StoreResult, blobs};

/// The format this code writes. Raise it with any change to `schema.rhype`
/// or to how the store fills it that an older store would not match.
pub(crate) const CURRENT: u32 = 3;

const INFO: &str = "StoreInfo";
const INFO_KEY: &str = "store";

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
        root: root.to_path_buf(),
        outbox_lock: Arc::default(),
        thread_lock: Arc::default(),
        writes: Arc::default(),
        stats: Arc::default(),
    };
    let key = Value::String(INFO_KEY.into());
    match store.db.find_by_unique(INFO, "key", &key)? {
        Some(info) => {
            let version = Fields::new(INFO, info).u32("version")?;
            if version != CURRENT {
                return Err(old(format!(
                    "it is format {version}, this Postrinch writes {CURRENT}"
                )));
            }
        }
        None if store.db.count_type("Account")? > 0 => {
            return Err(old("it predates format versions".into()));
        }
        None => {
            let fields = || {
                FieldsBuilder::new()
                    .str("key", INFO_KEY)
                    .u32("version", CURRENT)
            };
            store.upsert(INFO, "key", INFO_KEY, fields().build(), fields().build())?;
        }
    }
    Ok(store)
}

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
    fn a_new_store_records_the_current_format() {
        let dir = tempfile::tempdir().unwrap();
        drop(Store::open(dir.path()).unwrap());
        Store::open(dir.path()).unwrap();
    }

    #[test]
    fn another_format_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let fields = FieldsBuilder::new().u32("version", CURRENT + 1).build();
        let info = store.find(INFO, "key", INFO_KEY).unwrap().unwrap();
        store.db_update(INFO, info.id(), fields).unwrap();
        drop((info, store));
        assert!(matches!(
            Store::open(dir.path()),
            Err(StoreError::OldFormat { .. })
        ));
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
    fn a_store_from_before_the_outbox_opens() {
        let dir = tempfile::tempdir().unwrap();
        // A Windows checkout has CRLF line ends.
        let schema = SCHEMA.replace("\r\n", "\n");
        let start = schema.find("type OutboxEntry").expect("the outbox type");
        let comment = schema[..start].rfind("\n\n").expect("its comment");
        let before = &schema[..comment];
        let schema = rhypedb_schema::parser::parse_schema(before).unwrap();
        let db = Database::open(schema, dir.path().join("db")).unwrap();
        let fields = || {
            FieldsBuilder::new()
                .str("key", INFO_KEY)
                .u32("version", CURRENT)
        };
        db.create(INFO, fields().build()).unwrap();
        drop(db);
        Store::open(dir.path()).expect("an added type needs no new format");
    }
}
