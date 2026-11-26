//! Social (OFSCP) accounts and what they hold: groups, places, views,
//! messages, media, drafts, notifications and the outbox. See
//! docs/ofscp.md §"Storage (rhypedb)" and the social types in
//! `schema.rhype`.
//!
//! As the calendar does, each object keeps its record whole as JSON in
//! `record` and repeats in scalar fields what is filtered or sorted on;
//! keys are `"<account>\u001f<id>"`, the account being its actor
//! lower-cased ([`social_account_key`]). Records name their account by its actor
//! and are found by its key in any spelling; a message reads back with its
//! account's key. Removing an account removes everything it holds
//! ([`Store::remove_social_account`]).

mod accounts;
mod drafts;
mod groups;
mod media;
mod messages;
mod notifications;
mod outbox;
mod places;
mod types;
mod views;

use rhypedb_engine::CompareOp;
use serde::de::DeserializeOwned;

pub use types::*;

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::SEP;
use crate::{Store, StoreResult};

/// The field each social object keeps its record in.
const RECORD: &str = "record";
/// The unique field of every social type but the provider.
const KEY: &str = "key";

/// The store's key for a social account: its actor (`handle@domain`)
/// lower-cased. Handles are lower-case already and domains compare without
/// case, so two spellings of one actor are one account.
pub fn social_account_key(actor: &str) -> String {
    actor.to_ascii_lowercase()
}

/// The key of `id` within an account.
fn scoped(account: &str, id: &str) -> String {
    format!("{}{SEP}{id}", social_account_key(account))
}

impl Store {
    /// The record of the `type_name` object whose unique `field` is `key`.
    fn social_record<T: DeserializeOwned>(
        &self,
        type_name: &'static str,
        field: &str,
        key: &str,
    ) -> StoreResult<Option<T>> {
        self.find(type_name, field, key)?
            .map(|found| found.json(RECORD))
            .transpose()
    }

    /// The `type_name` objects whose string `field` is `value`.
    fn social_where(
        &self,
        type_name: &'static str,
        field: &str,
        value: &str,
    ) -> StoreResult<Vec<Fields>> {
        let found = self
            .db
            .filter_scan_str(type_name, field, CompareOp::Eq, value, None)?;
        Ok(found
            .into_iter()
            .map(|o| Fields::new(type_name, o))
            .collect())
    }

    /// The records of the `type_name` objects whose string `field` is
    /// `value`, in no particular order.
    fn social_records_where<T: DeserializeOwned>(
        &self,
        type_name: &'static str,
        field: &str,
        value: &str,
    ) -> StoreResult<Vec<T>> {
        self.social_where(type_name, field, value)?
            .iter()
            .map(|found| found.json(RECORD))
            .collect()
    }

    /// Write the `type_name` object whose unique `field` is `key`, with
    /// `fields`, creating it if it is not here. Returns its id.
    fn social_put(
        &self,
        type_name: &'static str,
        field: &str,
        key: &str,
        fields: FieldsBuilder,
    ) -> StoreResult<u64> {
        let create = fields.clone().str(field, key).build();
        self.upsert(type_name, field, key, create, fields.build())
    }

    /// Delete the `type_name` object whose unique `field` is `key`.
    /// Returns whether one was here.
    fn social_remove(&self, type_name: &'static str, field: &str, key: &str) -> StoreResult<bool> {
        match self.find(type_name, field, key)? {
            Some(found) => {
                self.db_delete(type_name, found.id())?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Delete every `type_name` object whose string `field` is `value`.
    /// Returns how many went.
    fn social_remove_where(
        &self,
        type_name: &'static str,
        field: &str,
        value: &str,
    ) -> StoreResult<usize> {
        let ids: Vec<u64> = self
            .social_where(type_name, field, value)?
            .iter()
            .map(Fields::id)
            .collect();
        for &id in &ids {
            self.db_delete(type_name, id)?;
        }
        Ok(ids.len())
    }

    /// Run `change` while no other read-modify-write of a social object
    /// runs.
    fn social_locked<T>(&self, change: impl FnOnce() -> StoreResult<T>) -> StoreResult<T> {
        let _held = self.social_lock.lock().unwrap_or_else(|e| e.into_inner());
        change()
    }
}

#[cfg(test)]
mod tests;
