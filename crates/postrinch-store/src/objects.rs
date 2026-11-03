//! Generic operations on stored objects: every engine write (retried when it
//! loses a write-write conflict), finding by a unique key, upserting, and
//! making a relation's links match a wanted set.

use std::collections::BTreeSet;

use std::time::Duration;

use rhypedb_engine::object::{FieldMap, Value};
use rhypedb_engine::{EngineError, EngineResult};

use crate::codec::Fields;
use crate::{Store, StoreError, StoreResult};

/// How many times a write is tried before a conflict is returned.
const CONFLICT_ATTEMPTS: u32 = 20;

impl Store {
    /// The object of `type_name` whose unique `field` is `key`.
    pub(crate) fn find(
        &self,
        type_name: &'static str,
        field: &str,
        key: &str,
    ) -> StoreResult<Option<Fields>> {
        let found = self
            .db
            .find_by_unique(type_name, field, &Value::String(key.to_owned()))?;
        Ok(found.map(|o| Fields::new(type_name, o)))
    }

    pub(crate) fn get(&self, type_name: &'static str, id: u64) -> StoreResult<Fields> {
        Ok(Fields::new(type_name, self.db.get(type_name, id)?))
    }

    pub(crate) fn get_many(
        &self,
        type_name: &'static str,
        ids: &[u64],
    ) -> StoreResult<Vec<Fields>> {
        let objects = self.db.get_many(type_name, ids)?;
        Ok(objects
            .into_iter()
            .map(|o| Fields::new(type_name, o))
            .collect())
    }

    /// Create the object whose unique `field` is `key`, or update it if it
    /// exists. `create` is only used for a new object, so it can carry the
    /// to-one relations that are fixed at creation; `update` is applied to
    /// an existing one. Returns the object's id.
    ///
    /// Two writers racing to create the same key both succeed: the loser's
    /// unique violation turns into an update of the winner's object.
    pub(crate) fn upsert(
        &self,
        type_name: &'static str,
        field: &str,
        key: &str,
        create: FieldMap,
        update: FieldMap,
    ) -> StoreResult<u64> {
        if let Some(existing) = self.find(type_name, field, key)? {
            self.db_update(type_name, existing.id(), update)?;
            return Ok(existing.id());
        }
        match self.db_create(type_name, create) {
            Ok(id) => Ok(id),
            Err(StoreError::Engine(EngineError::UniqueViolation { .. })) => {
                let existing = self
                    .find(type_name, field, key)?
                    .expect("a unique violation means the key exists");
                self.db_update(type_name, existing.id(), update)?;
                Ok(existing.id())
            }
            Err(e) => Err(e),
        }
    }

    /// The ids `relation` on object `id` links to.
    pub(crate) fn links(&self, type_name: &str, id: u64, relation: &str) -> StoreResult<Vec<u64>> {
        Ok(self
            .db
            .get_links(type_name, id, relation)?
            .into_iter()
            .map(|(target, _)| target)
            .collect())
    }

    /// The single object a to-one `relation` links to, if any.
    pub(crate) fn link(
        &self,
        type_name: &str,
        id: u64,
        relation: &str,
    ) -> StoreResult<Option<u64>> {
        Ok(self.links(type_name, id, relation)?.into_iter().next())
    }

    /// Link and unlink so that `relation` on object `id` links to exactly
    /// `wanted`.
    pub(crate) fn set_links(
        &self,
        type_name: &str,
        id: u64,
        relation: &str,
        wanted: &[u64],
    ) -> StoreResult<()> {
        let have: BTreeSet<u64> = self.links(type_name, id, relation)?.into_iter().collect();
        let want: BTreeSet<u64> = wanted.iter().copied().collect();
        for target in want.difference(&have) {
            self.db_link(type_name, id, relation, *target)?;
        }
        for target in have.difference(&want) {
            self.db_unlink(type_name, id, relation, *target)?;
        }
        Ok(())
    }

    pub(crate) fn db_create(&self, type_name: &str, fields: FieldMap) -> StoreResult<u64> {
        self.retrying(|| self.db.create(type_name, fields.clone()).map(|o| o.id))
    }

    pub(crate) fn db_update(&self, type_name: &str, id: u64, fields: FieldMap) -> StoreResult<()> {
        self.retrying(|| self.db.update(type_name, id, fields.clone()).map(drop))
    }

    pub(crate) fn db_delete(&self, type_name: &str, id: u64) -> StoreResult<()> {
        self.retrying(|| self.db.delete(type_name, id))
    }

    pub(crate) fn db_link(
        &self,
        type_name: &str,
        id: u64,
        relation: &str,
        target: u64,
    ) -> StoreResult<()> {
        self.retrying(|| self.db.link(type_name, id, relation, target, None))
    }

    pub(crate) fn db_unlink(
        &self,
        type_name: &str,
        id: u64,
        relation: &str,
        target: u64,
    ) -> StoreResult<()> {
        self.retrying(|| self.db.unlink(type_name, id, relation, target))
    }

    /// Run one engine write. Each is a single transaction, and one that
    /// loses a write-write conflict to a concurrent writer (two messages
    /// linking the same person at once) did not commit, so it is run again,
    /// after a short, growing pause.
    fn retrying<T>(&self, mut write: impl FnMut() -> EngineResult<T>) -> StoreResult<T> {
        let mut pause = Duration::from_millis(1);
        for _ in 1..CONFLICT_ATTEMPTS {
            match write() {
                Err(EngineError::WriteConflict) => {
                    std::thread::sleep(pause);
                    pause = (pause * 2).min(Duration::from_millis(50));
                }
                other => return Ok(other?),
            }
        }
        Ok(write()?)
    }
}
