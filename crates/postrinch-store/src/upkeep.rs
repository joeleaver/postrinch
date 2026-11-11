//! Keeping the database's files in shape: one rewrite of old links and a
//! full compaction after an upgrade of the engine that changed how it
//! writes them.
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
/// upgrade needs one more: 1, the files that say what opening needs
/// (rhypedb 2d53f5f); 2, the files that list the objects they hold, so
/// opening reads no object data (rhypedb ab8a3ed); 3, links without the
/// whole-object copies older engines wrote into them, which lists read
/// (rhypedb 122fd73, rewritten by `rewrite_covers` before the compaction).
const LAYOUT: u32 = 3;
/// How many times the link rewrite runs when it loses a write to a
/// concurrent writer; each run carries on where the last stopped.
const REWRITE_ATTEMPTS: usize = 5;

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
        let mut attempt = 1;
        loop {
            match self.db.rewrite_covers() {
                Ok(stats) => {
                    tracing::info!(
                        "rewrote {} links and {} index entries",
                        stats.rev_edges_rewritten,
                        stats.index_entries_rewritten
                    );
                    break;
                }
                Err(EngineError::WriteConflict) if attempt < REWRITE_ATTEMPTS => attempt += 1,
                Err(e) => return Err(e.into()),
            }
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
        self.release_memory();
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
