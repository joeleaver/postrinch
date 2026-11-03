//! Reading typed values out of stored objects, and building field maps.

use rhypedb_engine::object::{FieldMap, Object, Value};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{StoreError, StoreResult};

/// A stored object's fields, read with the type the schema promises. A
/// field that is missing or of the wrong type is [`StoreError::Corrupt`].
pub(crate) struct Fields {
    type_name: &'static str,
    object: Object,
}

impl Fields {
    pub(crate) fn new(type_name: &'static str, mut object: Object) -> Self {
        object.ensure_fields_deserialized();
        Self { type_name, object }
    }

    pub(crate) fn id(&self) -> u64 {
        self.object.id
    }

    fn value(&self, name: &str) -> &Value {
        self.object.fields.get(name).unwrap_or(&Value::Null)
    }

    fn wrong(&self, name: &str, expected: &str) -> StoreError {
        StoreError::corrupt(
            self.type_name,
            self.object.id,
            format!(
                "{name} is {}, expected {expected}",
                self.value(name).type_name()
            ),
        )
    }

    pub(crate) fn opt_str(&self, name: &str) -> StoreResult<Option<String>> {
        match self.value(name) {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s.clone())),
            _ => Err(self.wrong(name, "String")),
        }
    }

    pub(crate) fn str(&self, name: &str) -> StoreResult<String> {
        self.opt_str(name)?
            .ok_or_else(|| self.wrong(name, "String"))
    }

    pub(crate) fn opt_bool(&self, name: &str) -> StoreResult<Option<bool>> {
        match self.value(name) {
            Value::Null => Ok(None),
            Value::Bool(b) => Ok(Some(*b)),
            _ => Err(self.wrong(name, "Bool")),
        }
    }

    pub(crate) fn bool(&self, name: &str) -> StoreResult<bool> {
        match self.value(name) {
            Value::Bool(b) => Ok(*b),
            _ => Err(self.wrong(name, "Bool")),
        }
    }

    pub(crate) fn u64(&self, name: &str) -> StoreResult<u64> {
        match self.value(name) {
            Value::U64(n) => Ok(*n),
            _ => Err(self.wrong(name, "u64")),
        }
    }

    pub(crate) fn datetime(&self, name: &str) -> StoreResult<i64> {
        match self.value(name) {
            Value::DateTime(ms) => Ok(*ms),
            _ => Err(self.wrong(name, "DateTime")),
        }
    }

    pub(crate) fn json<T: DeserializeOwned>(&self, name: &str) -> StoreResult<T> {
        match self.value(name) {
            Value::Json(v) => serde_json::from_value(v.clone()).map_err(|e| {
                StoreError::corrupt(self.type_name, self.object.id, format!("{name}: {e}"))
            }),
            _ => Err(self.wrong(name, "Json")),
        }
    }
}

/// Builds a [`FieldMap`] for a create or an update.
#[derive(Default, Clone)]
pub(crate) struct FieldsBuilder(FieldMap);

impl FieldsBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn str(mut self, name: &str, value: impl Into<String>) -> Self {
        self.0.insert(name.into(), Value::String(value.into()));
        self
    }

    pub(crate) fn opt_str(mut self, name: &str, value: Option<impl Into<String>>) -> Self {
        let value = value.map_or(Value::Null, |v| Value::String(v.into()));
        self.0.insert(name.into(), value);
        self
    }

    pub(crate) fn bool(mut self, name: &str, value: bool) -> Self {
        self.0.insert(name.into(), Value::Bool(value));
        self
    }

    pub(crate) fn u64(mut self, name: &str, value: u64) -> Self {
        self.0.insert(name.into(), Value::U64(value));
        self
    }

    pub(crate) fn datetime(mut self, name: &str, millis: i64) -> Self {
        self.0.insert(name.into(), Value::DateTime(millis));
        self
    }

    pub(crate) fn json(mut self, name: &str, value: &impl Serialize) -> Self {
        let json = serde_json::to_value(value).expect("model types serialize to JSON");
        self.0.insert(name.into(), Value::Json(json));
        self
    }

    /// A to-one relation, written in the same transaction as the create.
    pub(crate) fn relation(mut self, name: &str, target_id: u64) -> Self {
        self.0.insert(name.into(), Value::U64(target_id));
        self
    }

    pub(crate) fn build(self) -> FieldMap {
        self.0
    }
}
