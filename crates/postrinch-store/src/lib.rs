//! Postrinch's local store.
//!
//! Every read and write of the rhypedb database lives here, behind [`Store`];
//! no other crate names a rhypedb type. The one place the schema shows
//! through is a saved view's query, which is written in rhypedb's query
//! language against the `Message` type in `schema.rhype`. Raw messages are kept beside
//! the database in a content-addressed blob directory, because a message
//! with attachments can run to megabytes and has no business in the LSM tree.

mod accounts;
mod blobs;
mod codec;
mod error;
mod keys;
mod labels;
mod messages;
mod objects;
mod people;
mod search;
mod views;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rhypedb_engine::database::Database;

pub use error::{StoreError, StoreResult};
pub use search::SearchHit;

const SCHEMA: &str = include_str!("schema.rhype");

/// The store: one rhypedb database and one blob directory under a root.
///
/// Cheap to clone; clones share the open database. Every method is a
/// complete operation the caller can retry: none leaves the store in a
/// state a repeat of the same call does not repair.
#[derive(Clone)]
pub struct Store {
    db: Arc<Database>,
    blobs: blobs::Blobs,
    root: PathBuf,
}

impl Store {
    /// Open the store under `root`, creating it if it does not exist.
    pub fn open(root: impl AsRef<Path>) -> StoreResult<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let schema = rhypedb_schema::parser::parse_schema(SCHEMA)
            .map_err(|e| StoreError::Schema(e.to_string()))?;
        let db = Database::open(schema, root.join("db"))?;
        let blobs = blobs::Blobs::open(root.join("blobs"))?;
        Ok(Self { db, blobs, root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Delete raw messages no stored message refers to any more (after a
    /// message or an account is removed). Blobs written in the last
    /// `grace` are kept, for writers still between storing a blob and the
    /// message that refers to it. Returns how many were deleted.
    pub fn collect_garbage(&self, grace: Duration) -> StoreResult<usize> {
        let referenced: std::collections::HashSet<String> = self
            .db
            .scan_type_projected(messages::TYPE, &["raw_sha256"])?
            .into_iter()
            .map(|o| codec::Fields::new(messages::TYPE, o))
            .map(|f| f.opt_str("raw_sha256"))
            .filter_map(|r| r.transpose())
            .collect::<StoreResult<_>>()?;
        Ok(self.blobs.retain(|hash| referenced.contains(hash), grace)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_parses() {
        rhypedb_schema::parser::parse_schema(SCHEMA).expect("schema.rhype parses");
    }

    #[test]
    fn opens_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        drop(Store::open(dir.path()).unwrap());
        Store::open(dir.path()).unwrap();
    }
}
