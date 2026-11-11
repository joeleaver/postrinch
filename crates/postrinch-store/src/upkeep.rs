//! Keeping the database's files in shape: one full compaction after an
//! upgrade of the engine that changed how it writes them.
//!
//! rhypedb writes its newer files with what opening needs to know about
//! them; files from before are read in full at every open until a
//! compaction rewrites them. The background compaction rewrites a store's
//! biggest file only rarely, so the first open after such an upgrade
//! compacts everything once, and says so in the store.

use rhypedb_engine::EngineError;
use rhypedb_engine::object::Value;

use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreResult};

const INFO: &str = "StoreInfo";
const COMPACTED_KEY: &str = "compacted";
/// The file layout the last full compaction wrote. Raise it when an engine
/// upgrade needs one more.
const LAYOUT: u32 = 1;

impl Store {
    /// Whether the store has not yet had the full compaction its files need.
    pub fn needs_compaction(&self) -> StoreResult<bool> {
        let key = Value::String(COMPACTED_KEY.into());
        Ok(match self.db.find_by_unique(INFO, "key", &key)? {
            Some(info) => Fields::new(INFO, info).u32("version")? < LAYOUT,
            None => true,
        })
    }

    /// Compact the whole database once, if it has not been since the
    /// engine changed how it writes its files, then give the memory it took
    /// back to the system. Returns whether it compacted. Takes a while on a
    /// big store: run it off the window's thread.
    pub fn compact_once(&self) -> StoreResult<bool> {
        if !self.needs_compaction()? {
            return Ok(false);
        }
        self.db.storage().compact().map_err(EngineError::from)?;
        let fields = || {
            FieldsBuilder::new()
                .str("key", COMPACTED_KEY)
                .u32("version", LAYOUT)
        };
        self.upsert(
            INFO,
            "key",
            COMPACTED_KEY,
            fields().build(),
            fields().build(),
        )?;
        self.db.release_free_memory();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(store.needs_compaction().unwrap());
        assert!(store.compact_once().unwrap());
        assert!(!store.needs_compaction().unwrap());
        assert!(!store.compact_once().unwrap());
    }
}
