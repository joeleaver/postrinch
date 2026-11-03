//! Generic operations on stored objects: finding by a unique key, upserting,
//! and making a relation's links match a wanted set.

use std::collections::BTreeSet;

use rhypedb_engine::EngineError;
use rhypedb_engine::object::{FieldMap, Value};

use crate::codec::Fields;
use crate::{Store, StoreResult};

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
            self.db.update(type_name, existing.id(), update)?;
            return Ok(existing.id());
        }
        match self.db.create(type_name, create) {
            Ok(object) => Ok(object.id),
            Err(EngineError::UniqueViolation { .. }) => {
                let existing = self
                    .find(type_name, field, key)?
                    .expect("a unique violation means the key exists");
                self.db.update(type_name, existing.id(), update)?;
                Ok(existing.id())
            }
            Err(e) => Err(e.into()),
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
            self.db.link(type_name, id, relation, *target, None)?;
        }
        for target in have.difference(&want) {
            self.db.unlink(type_name, id, relation, *target)?;
        }
        Ok(())
    }
}
