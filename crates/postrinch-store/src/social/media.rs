//! What is known of the pictures, videos and files messages carry, and
//! their bytes once fetched (in the blob store, as mail's raw messages).

use super::{KEY, RECORD, SocialMedia, scoped, social_account_key};
use crate::codec::{Fields, FieldsBuilder};
use crate::keys::SEP;
use crate::{Store, StoreResult};

const MEDIA: &str = "SocialMedia";

fn media_key(account: &str, provider: &str, id: &str) -> String {
    let provider = provider.to_ascii_lowercase();
    scoped(account, &format!("{provider}{SEP}{id}"))
}

impl Store {
    /// Keep what is known of an attachment, replacing what was kept.
    pub fn put_social_media(&self, media: &SocialMedia) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .str("account", social_account_key(&media.account))
            .str("url", media.url.clone())
            .opt_str("blob", media.blob.clone())
            .opt_datetime("fetched", media.fetched)
            .json(RECORD, media);
        let key = media_key(&media.account, &media.provider, &media.id);
        self.social_put(MEDIA, KEY, &key, fields)?;
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

    /// Keep fetched or picked bytes in the blob store, returning their
    /// SHA-256 for a [`SocialMedia::blob`] or a kept draft picture. Kept
    /// while media or a draft refers to them (see
    /// [`Store::collect_garbage`]).
    pub fn put_social_media_bytes(&self, bytes: &[u8]) -> StoreResult<String> {
        Ok(self.blobs.put(bytes)?)
    }

    /// The bytes kept under `blob`, if they are still here.
    pub fn social_media_bytes(&self, blob: &str) -> StoreResult<Option<Vec<u8>>> {
        Ok(self.blobs.get(blob)?)
    }

    /// Forget an attachment. Its bytes stay until no one refers to them
    /// and [`Store::collect_garbage`] runs.
    pub fn remove_social_media(&self, account: &str, provider: &str, id: &str) -> StoreResult<()> {
        self.social_remove(MEDIA, KEY, &media_key(account, provider, id))?;
        Ok(())
    }

    /// Forget every attachment `account` knows.
    pub(super) fn remove_social_media_of(&self, account: &str) -> StoreResult<usize> {
        self.social_remove_where(MEDIA, "account", &social_account_key(account))
    }

    /// Every blob social media refers to.
    pub(crate) fn social_media_blobs(&self) -> StoreResult<Vec<String>> {
        let mut blobs = Vec::new();
        for object in self.db.scan_type_projected(MEDIA, &["blob"])? {
            blobs.extend(Fields::new(MEDIA, object).opt_str("blob")?);
        }
        Ok(blobs)
    }
}
