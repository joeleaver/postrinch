//! Changes made here on their way to a provider, in the order they were
//! made. Each is kept until its provider has taken it; reads lay the
//! pending ones over what sync stored, so sync never undoes one.

use super::{
    DraftMedia, KEY, RECORD, SocialChange, SocialOutboxEntry, UploadedFile, scoped,
    social_account_key,
};
use crate::codec::{Fields, FieldsBuilder};
use crate::format::{INFO, INFO_KEY};
use crate::{Store, StoreError, StoreResult};

const OUTBOX: &str = "SocialOutbox";

impl Store {
    /// Queue a change, as made at `created`, to go out after every change
    /// queued before it. The blobs of the files it uploads first are kept
    /// until it is gone ([`SocialChange::blobs`]). Queuing a client id that is queued already returns that entry as it
    /// is: a repeat of the call queues nothing twice.
    pub fn queue_social_change(
        &self,
        account: &str,
        client_id: &str,
        place: Option<&str>,
        change: &SocialChange,
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
                tried: false,
                attempts: 0,
                error: None,
                refused: false,
                uploads: Vec::new(),
            };
            self.write_social_change(&key, &entry)?;
            Ok(entry)
        })
    }

    /// The next sequence number, never one taken before: the last one is
    /// kept in the store's info, so numbers go on rising once the outbox
    /// is empty. Called with the social lock held.
    fn next_social_seq(&self) -> StoreResult<u64> {
        let info = self
            .find(INFO, "key", INFO_KEY)?
            .ok_or_else(|| StoreError::corrupt(INFO, 0, "the store's info is missing"))?;
        let next = info.opt_u64("social_seq")?.unwrap_or(0) + 1;
        let fields = FieldsBuilder::new().u64("social_seq", next).build();
        self.db_update(INFO, info.id(), fields)?;
        Ok(next)
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

    /// Record that the engine is about to send a queued change: from then
    /// on it may reach its provider ([`Store::take_back_social_change`]).
    /// Returns the entry as it is now, or `None` when it is not queued.
    pub fn social_change_tried(
        &self,
        account: &str,
        client_id: &str,
    ) -> StoreResult<Option<SocialOutboxEntry>> {
        let key = scoped(account, client_id);
        self.social_locked(|| {
            let Some(mut entry) = self.social_record::<SocialOutboxEntry>(OUTBOX, KEY, &key)?
            else {
                return Ok(None);
            };
            if !entry.tried {
                entry.tried = true;
                self.write_social_change(&key, &entry)?;
            }
            Ok(Some(entry))
        })
    }

    /// Take back a queued change the engine has not begun to send, as if
    /// it was never made. Deciding and removing are one step, so the
    /// engine never sends what was taken back.
    pub fn take_back_social_change(&self, account: &str, client_id: &str) -> StoreResult<TakeBack> {
        let key = scoped(account, client_id);
        self.social_locked(
            || match self.social_record::<SocialOutboxEntry>(OUTBOX, KEY, &key)? {
                None => Ok(TakeBack::NotQueued),
                Some(entry) if entry.tried || entry.attempts > 0 => Ok(TakeBack::Tried),
                Some(_) => {
                    self.social_remove(OUTBOX, KEY, &key)?;
                    Ok(TakeBack::Taken)
                }
            },
        )
    }

    /// Forget a queued post that sync has kept as `place`'s message: its
    /// provider has it. Called with the social lock held, as the message
    /// is written. Returns whether a post was queued there.
    pub(super) fn settle_social_post(
        &self,
        account: &str,
        place: &str,
        client_id: &str,
    ) -> StoreResult<bool> {
        let key = scoped(account, client_id);
        if let Some(entry) = self.social_record::<SocialOutboxEntry>(OUTBOX, KEY, &key)?
            && matches!(entry.change, SocialChange::Post(_))
            && entry.place.as_deref() == Some(place)
        {
            self.social_remove(OUTBOX, KEY, &key)?;
            return Ok(true);
        }
        Ok(false)
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

    /// Record that `media`, a file a queued change carries, was uploaded
    /// at `at` as `attachment` (the Attachment its provider gave back,
    /// whole), replacing an older upload of it: a retry sends that, and
    /// does not upload it again while it is fresh
    /// ([`SocialOutboxEntry::uploads`]). Returns the entry as it is now,
    /// or `None` when it is not queued (it was taken back while the file
    /// went).
    pub fn social_change_uploaded(
        &self,
        account: &str,
        client_id: &str,
        media: &DraftMedia,
        attachment: serde_json::Value,
        at: i64,
    ) -> StoreResult<Option<SocialOutboxEntry>> {
        let key = scoped(account, client_id);
        self.social_locked(|| {
            let Some(mut entry) = self.social_record::<SocialOutboxEntry>(OUTBOX, KEY, &key)?
            else {
                return Ok(None);
            };
            entry.uploads.retain(|u| u.media != *media);
            entry.uploads.push(UploadedFile {
                media: media.clone(),
                attachment,
                at,
            });
            self.write_social_change(&key, &entry)?;
            Ok(Some(entry))
        })
    }

    /// Record that uploading a queued change's files failed for now, and
    /// why. The change was not sent, so this is not an attempt
    /// ([`SocialOutboxEntry::attempts`]): it can still be taken back.
    /// Returns the entry as it is now, or `None` when it is not queued.
    pub fn social_change_not_uploaded(
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
            entry.error = Some(error.to_owned());
            self.write_social_change(&key, &entry)?;
            Ok(Some(entry))
        })
    }

    /// Record that the provider refused a queued change for good, and why:
    /// it is kept, for the person to see and give up, but not tried
    /// again. Returns the entry as it is now, or `None` when it is not
    /// queued.
    pub fn social_change_refused(
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
            entry.refused = true;
            self.write_social_change(&key, &entry)?;
            Ok(Some(entry))
        })
    }

    /// Forget a change its provider has taken (or the person gave up on).
    /// Returns whether it was queued.
    pub fn remove_social_change(&self, account: &str, client_id: &str) -> StoreResult<bool> {
        self.social_locked(|| self.social_remove(OUTBOX, KEY, &scoped(account, client_id)))
    }

    /// Every blob a queued change uploads.
    pub(crate) fn social_outbox_blobs(&self) -> StoreResult<Vec<String>> {
        let mut blobs = Vec::new();
        for object in self.db.scan_type_projected(OUTBOX, &[RECORD])? {
            let entry: SocialOutboxEntry = Fields::new(OUTBOX, object).json(RECORD)?;
            blobs.extend(entry.change.blobs());
        }
        Ok(blobs)
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

/// What came of taking back a queued change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeBack {
    /// It was taken back: nothing will be sent.
    Taken,
    /// The engine has begun sending it, so it may reach its provider.
    Tried,
    /// It is not queued (sent, given up, or never made).
    NotQueued,
}
