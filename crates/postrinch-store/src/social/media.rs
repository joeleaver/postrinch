//! What is known of the pictures, videos and files messages carry (and
//! of avatars), and the bytes picked or pasted to upload. Fetched bytes
//! are in the media cache (media_cache.rs).

use super::{KEY, RECORD, SocialMedia, scoped, social_account_key};
use crate::codec::{Fields, FieldsBuilder};
use crate::keys::SEP;
use crate::{Store, StoreResult};

pub(super) const MEDIA: &str = "SocialMedia";

pub(super) fn media_key(account: &str, provider: &str, id: &str) -> String {
    let provider = provider.to_ascii_lowercase();
    scoped(account, &format!("{provider}{SEP}{id}"))
}

impl Store {
    /// Keep what is known of an attachment, replacing what was kept.
    /// Cached bytes the record named before, and no record names now, go
    /// from the cache.
    pub fn put_social_media(&self, media: &SocialMedia) -> StoreResult<()> {
        // Under the social lock, as every change of the records and the
        // cache is (media_cache.rs).
        self.social_locked(|| self.write_social_media(media))
    }

    /// [`Store::put_social_media`]; called with the social lock held.
    pub(super) fn write_social_media(&self, media: &SocialMedia) -> StoreResult<()> {
        let key = media_key(&media.account, &media.provider, &media.id);
        let before: Option<SocialMedia> = self.social_record(MEDIA, KEY, &key)?;
        let fields = FieldsBuilder::new()
            .str("account", social_account_key(&media.account))
            .str("url", media.url.clone())
            .opt_str("blob", media.blob.clone())
            .opt_datetime("fetched", media.fetched)
            .json(RECORD, media);
        self.social_put(MEDIA, KEY, &key, fields)?;
        if let Some(old) = before.and_then(|b| b.blob)
            && media.blob.as_deref() != Some(old.as_str())
        {
            self.release_cached(&old)?;
        }
        Ok(())
    }

    /// The attachment `id` of the provider at `provider`, as `account`
    /// knows it.
    pub fn social_media(
        &self,
        account: &str,
        provider: &str,
        id: &str,
    ) -> StoreResult<Option<SocialMedia>> {
        self.social_record(MEDIA, KEY, &media_key(account, provider, id))
    }

    /// The attachments fetched from `url`, for every account.
    pub fn social_media_at(&self, url: &str) -> StoreResult<Vec<SocialMedia>> {
        let mut found: Vec<SocialMedia> = self.social_records_where(MEDIA, "url", url)?;
        found.sort_by(|a, b| {
            (&a.account, &a.provider, &a.id).cmp(&(&b.account, &b.provider, &b.id))
        });
        Ok(found)
    }

    /// Keep picked or pasted bytes (a draft's picture, a new avatar) in
    /// the blob store, returning their SHA-256 for a
    /// [`DraftMedia::Kept`](super::DraftMedia::Kept). Kept while a draft or
    /// a queued change refers to them (see [`Store::collect_garbage`]).
    pub fn put_social_kept_bytes(&self, bytes: &[u8]) -> StoreResult<String> {
        Ok(self.blobs.put(bytes)?)
    }

    /// The picked or pasted bytes kept under `blob`, if they are still
    /// here.
    pub fn social_kept_bytes(&self, blob: &str) -> StoreResult<Option<Vec<u8>>> {
        Ok(self.blobs.get(blob)?)
    }

    /// Forget an attachment. Its cached bytes stay until no record refers
    /// to them and [`Store::collect_garbage`] runs.
    pub fn remove_social_media(&self, account: &str, provider: &str, id: &str) -> StoreResult<()> {
        self.social_locked(|| {
            let key = media_key(account, provider, id);
            let before: Option<SocialMedia> = self.social_record(MEDIA, KEY, &key)?;
            self.social_remove(MEDIA, KEY, &key)?;
            if let Some(blob) = before.and_then(|b| b.blob) {
                self.release_cached(&blob)?;
            }
            Ok(())
        })
    }

    /// Forget every attachment `account` knows, and the cached bytes no
    /// other account's record names.
    pub(super) fn remove_social_media_of(&self, account: &str) -> StoreResult<usize> {
        self.social_locked(|| {
            let account = social_account_key(account);
            let records: Vec<SocialMedia> =
                self.social_records_where(MEDIA, "account", &account)?;
            let removed = self.social_remove_where(MEDIA, "account", &account)?;
            for blob in records.into_iter().filter_map(|r| r.blob) {
                self.release_cached(&blob)?;
            }
            Ok(removed)
        })
    }

    /// Every blob of the media cache an attachment refers to.
    pub(crate) fn social_media_blobs(&self) -> StoreResult<Vec<String>> {
        let mut blobs = Vec::new();
        for object in self.db.scan_type_projected(MEDIA, &["blob"])? {
            blobs.extend(Fields::new(MEDIA, object).opt_str("blob")?);
        }
        Ok(blobs)
    }
}
