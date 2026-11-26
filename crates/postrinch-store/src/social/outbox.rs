//! Changes made here on their way to a provider, in the order they were
//! made. Each is kept until its provider has taken it; reads lay the
//! pending ones over what sync stored, so sync never undoes one.

use serde_json::Value;

use super::{KEY, RECORD, SocialOutboxEntry, scoped, social_account_key};
use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreResult};

const OUTBOX: &str = "SocialOutbox";

impl Store {
    /// Queue a change, as made at `created`, to go out after every change
    /// queued before it. Queuing a client id that is queued already returns
    /// that entry as it is: a repeat of the call queues nothing twice.
    pub fn queue_social_change(
        &self,
        account: &str,
        client_id: &str,
        place: Option<&str>,
        change: &Value,
        created: i64,
    ) -> StoreResult<SocialOutboxEntry> {
        let key = scoped(account, client_id);
        self.social_locked(|| {
            if let Some(queued) = self.social_record(OUTBOX, KEY, &key)? {
                return Ok(queued);
            }
            let entry = SocialOutboxEntry {
                account: social_account_key(account),
                client_id: client_id.to_owned(),
                place: place.map(str::to_owned),
                seq: self.next_social_seq()?,
                change: change.clone(),
                created,
                attempts: 0,
                error: None,
            };
            self.write_social_change(&key, &entry)?;
            Ok(entry)
        })
    }

    fn next_social_seq(&self) -> StoreResult<u64> {
        let mut last = 0;
        for object in self.db.scan_type_projected(OUTBOX, &["seq"])? {
            last = last.max(Fields::new(OUTBOX, object).u64("seq")?);
        }
        Ok(last + 1)
    }

    fn write_social_change(&self, key: &str, entry: &SocialOutboxEntry) -> StoreResult<()> {
        let place = entry.place.as_ref().map(|p| scoped(&entry.account, p));
        let fields = FieldsBuilder::new()
            .str("account", social_account_key(&entry.account))
            .opt_str("place", place)
            .u64("seq", entry.seq)
            .json(RECORD, entry);
        self.social_put(OUTBOX, KEY, key, fields)?;
        Ok(())
    }

    /// An account's queued changes, in the order they were made.
    pub fn social_outbox(&self, account: &str) -> StoreResult<Vec<SocialOutboxEntry>> {
        let entries = self.social_records_where(OUTBOX, "account", &social_account_key(account))?;
        Ok(in_order(entries))
    }

    /// The queued changes to one place, in the order they were made.
    pub fn social_outbox_for(
        &self,
        account: &str,
        place: &str,
    ) -> StoreResult<Vec<SocialOutboxEntry>> {
        let entries = self.social_records_where(OUTBOX, "place", &scoped(account, place))?;
        Ok(in_order(entries))
    }

    /// Record an attempt to send a queued change that failed, and why.
    /// Returns the entry as it is now, or `None` when it is not queued.
    pub fn social_change_failed(
        &self,
        account: &str,
        client_id: &str,
        error: &str,
    ) -> StoreResult<Option<SocialOutboxEntry>> {
        let key = scoped(account, client_id);
        self.social_locked(|| {
            let Some(mut entry) = self.social_record::<SocialOutboxEntry>(OUTBOX, KEY, &key)?
            else {
                return Ok(None);
            };
            entry.attempts += 1;
            entry.error = Some(error.to_owned());
            self.write_social_change(&key, &entry)?;
            Ok(Some(entry))
        })
    }

    /// Forget a change its provider has taken (or the person gave up on).
    /// Returns whether it was queued.
    pub fn remove_social_change(&self, account: &str, client_id: &str) -> StoreResult<bool> {
        self.social_locked(|| self.social_remove(OUTBOX, KEY, &scoped(account, client_id)))
    }

    /// Forget every queued change of `account`.
    pub(super) fn remove_social_outbox(&self, account: &str) -> StoreResult<usize> {
        self.social_locked(|| {
            self.social_remove_where(OUTBOX, "account", &social_account_key(account))
        })
    }

    /// Forget every queued change to the place whose key is `place`.
    pub(super) fn remove_social_outbox_of_place(&self, place: &str) -> StoreResult<usize> {
        self.social_locked(|| self.social_remove_where(OUTBOX, "place", place))
    }
}

/// Sorted by `seq`.
fn in_order(mut entries: Vec<SocialOutboxEntry>) -> Vec<SocialOutboxEntry> {
    entries.sort_by_key(|e| e.seq);
    entries
}
