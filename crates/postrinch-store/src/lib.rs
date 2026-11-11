//! Postrinch's local store.
//!
//! Every read and write of the rhypedb database lives here, behind [`Store`];
//! no other crate names a rhypedb type. The one place the schema shows
//! through is a saved view's query, which is written in rhypedb's query
//! language against the `Message` type in `schema.rhype`. Raw messages are kept beside
//! the database in a content-addressed blob directory, because a message
//! with attachments can run to megabytes and has no business in the LSM tree.

mod accounts;
mod backfill;
mod blobs;
mod codec;
mod duplicates;
mod error;
mod find;
mod format;
mod keys;
mod labels;
mod mailboxes;
mod messages;
mod objects;
mod outbox;
mod outgoing;
mod people;
mod threads;
mod upkeep;
mod views;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rhypedb_engine::database::Database;

pub use error::{StoreError, StoreResult};
pub use find::{Find, Found};
pub use mailboxes::{LabelStats, MessageEntry, ThreadEntry};
pub use outbox::OutboxEntry;
pub use outgoing::{Outgoing, OutgoingKind};

/// Label stats by account key, each with the write count it was read at.
/// Undownloaded messages by account key, and when they were counted.
type KeptCounts = (std::time::Instant, HashMap<String, usize>);
type KeptStats = HashMap<String, (u64, HashMap<String, LabelStats>)>;

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
    /// Held while an outbox entry takes the next sequence number.
    outbox_lock: Arc<Mutex<()>>,
    /// Held while a thread's latest activity is read and written, as sync
    /// stores several messages of one thread at once.
    thread_lock: Arc<Mutex<()>>,
    /// How many writes have gone through since the store opened.
    writes: Arc<AtomicU64>,
    /// Each account's label stats, with [`Store::writes`] as it was when
    /// they were read: good until the next write.
    stats: Arc<Mutex<KeptStats>>,
    /// How many messages each account has not downloaded, and when that
    /// was counted (see backfill.rs).
    undownloaded: Arc<Mutex<Option<KeptCounts>>>,
}

impl Store {
    /// Open the store under `root`, creating it if it does not exist. A
    /// store in another format is refused ([`StoreError::OldFormat`]).
    pub fn open(root: impl AsRef<Path>) -> StoreResult<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        format::open(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Give memory back to the system: the heap freed since the last time
    /// (the allocator keeps what loading lists and threads let go of) and
    /// the database files' pages mapped in by reads, which come back from
    /// the page cache when next read. Until then the process looks that
    /// much bigger.
    pub fn release_memory(&self) {
        self.db.release_free_memory();
        self.db.release_file_pages();
    }

    /// How many writes have gone through since the store opened: what was
    /// read when it had one value is out of date once it has another.
    pub fn writes(&self) -> u64 {
        self.writes.load(Ordering::Acquire)
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
