//! The social media cache: the bytes of fetched pictures, videos, files
//! and avatars, in a blob directory of their own (`social-media/`), kept
//! up to a size the engine chooses and evicted least recently used first.
//!
//! Only [`SocialMedia`] records refer to its blobs, so a blob no record
//! refers to may go at once. Evicting forgets the bytes, never the record:
//! what is known of the attachment stays, and its bytes are fetched again
//! when next asked for. Records and the cache are changed under the social
//! lock, so an eviction never leaves a record naming bytes it removed.

use std::collections::HashMap;

use super::media::{MEDIA, media_key};
use super::{KEY, RECORD, SocialMedia};
use crate::codec::Fields;
use crate::{Store, StoreResult};

/// What trimming the cache did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Trimmed {
    /// Cached files evicted.
    pub evicted: usize,
    /// The bytes they held.
    pub freed: u64,
    /// The bytes the cache holds now.
    pub size: u64,
}

/// One cached blob: its size and when it was last used, by any record.
struct Cached {
    size: u64,
    last_used: i64,
}

impl Store {
    /// Keep fetched bytes in the media cache, returning their SHA-256 (in
    /// hex) for a [`SocialMedia::blob`]. They stay while a record refers to
    /// them, until evicted ([`Store::trim_social_media`]).
    pub fn cache_social_media_bytes(&self, bytes: &[u8]) -> StoreResult<String> {
        Ok(self.media_cache.put(bytes)?)
    }

    /// The cached bytes `blob`, if they are still here.
    pub fn cached_social_media_bytes(&self, blob: &str) -> StoreResult<Option<Vec<u8>>> {
        Ok(self.media_cache.get(blob)?)
    }

    /// Record that `account`'s attachment `id` of `provider` was asked for
    /// at `now`, so the cache keeps it over what was used longer ago.
    pub fn touch_social_media(
        &self,
        account: &str,
        provider: &str,
        id: &str,
        now: i64,
    ) -> StoreResult<()> {
        self.social_locked(|| {
            let key = media_key(account, provider, id);
            if let Some(mut media) = self.social_record::<SocialMedia>(MEDIA, KEY, &key)? {
                media.used = Some(now);
                self.write_social_media(&media)?;
            }
            Ok(())
        })
    }

    /// The bytes the media cache holds: each cached blob counted once,
    /// however many records refer to it.
    pub fn social_media_cache_size(&self) -> StoreResult<u64> {
        let cached = self.social_locked(|| self.cached_blobs())?;
        Ok(cached.values().map(|c| c.size).sum())
    }

    /// Evict cached bytes, least recently used first, until the cache
    /// holds at most `max_bytes`. `keep` (the bytes just fetched) is never
    /// evicted, so the cache may stay over the limit by that much.
    pub fn trim_social_media(&self, max_bytes: u64, keep: Option<&str>) -> StoreResult<Trimmed> {
        self.social_locked(|| {
            let cached = self.cached_blobs()?;
            let mut size: u64 = cached.values().map(|c| c.size).sum();
            let mut trimmed = Trimmed::default();
            let mut oldest: Vec<(&String, &Cached)> = cached
                .iter()
                .filter(|(blob, _)| Some(blob.as_str()) != keep)
                .collect();
            oldest.sort_by(|a, b| (a.1.last_used, a.0).cmp(&(b.1.last_used, b.0)));
            for (blob, entry) in oldest {
                if size <= max_bytes {
                    break;
                }
                self.evict(blob)?;
                size -= entry.size;
                trimmed.evicted += 1;
                trimmed.freed += entry.size;
            }
            trimmed.size = size;
            Ok(trimmed)
        })
    }

    /// Evict the cached bytes `blob`: every record of them forgets them,
    /// and they are deleted.
    pub fn evict_social_media(&self, blob: &str) -> StoreResult<()> {
        self.social_locked(|| self.evict(blob))
    }

    /// Delete the cached bytes `blob` if no record names them now; called
    /// with the social lock held.
    pub(super) fn release_cached(&self, blob: &str) -> StoreResult<()> {
        let named: Vec<SocialMedia> = self.social_records_where(MEDIA, "blob", blob)?;
        if named.is_empty() {
            self.media_cache.remove(blob)?;
        }
        Ok(())
    }

    /// Evict `blob`; called with the social lock held.
    fn evict(&self, blob: &str) -> StoreResult<()> {
        let records: Vec<SocialMedia> = self.social_records_where(MEDIA, "blob", blob)?;
        for mut media in records {
            media.blob = None;
            media.fetched = None;
            media.used = None;
            self.write_social_media(&media)?;
        }
        self.media_cache.remove(blob)?;
        Ok(())
    }

    /// Every cached blob a record refers to; called with the social lock
    /// held.
    fn cached_blobs(&self) -> StoreResult<HashMap<String, Cached>> {
        let mut cached: HashMap<String, Cached> = HashMap::new();
        for object in self.db.scan_type_projected(MEDIA, &["blob", RECORD])? {
            let fields = Fields::new(MEDIA, object);
            if fields.opt_str("blob")?.is_none() {
                continue;
            }
            let media: SocialMedia = fields.json(RECORD)?;
            let Some(blob) = media.blob else { continue };
            let last_used = media.used.max(media.fetched).unwrap_or(0);
            let entry = cached.entry(blob).or_insert(Cached { size: 0, last_used });
            entry.size = entry.size.max(media.size.unwrap_or(0));
            entry.last_used = entry.last_used.max(last_used);
        }
        Ok(cached)
    }
}
