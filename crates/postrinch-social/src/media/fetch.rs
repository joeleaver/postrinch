//! Fetching a picture, video or file a message carries, through the media
//! cache (§5.8 *Serving attachments*).
//!
//! Each is known by the provider that issued it and its id there: an
//! attachment is uploaded to its author's home provider (§5.8 *Where to
//! upload*), so its issuer is the author's domain, and ids are unique per
//! issuer (§2.2). Asked for again, it is served from the cache:
//!
//! - by its record, while the bytes it names are still cached and match
//!   the attachment's `hash` (a short-lived URL that changed since is no
//!   reason to fetch again: the bytes at an upload never change);
//! - else by its `hash`, when the same bytes are cached for another record.
//!
//! Otherwise it is fetched unsigned by its URL, its `hash` checked when it
//! has one (attachments are untrusted, §5.8), kept, and the cache trimmed
//! to its size. A fetch's bytes are a claim of their host: the engine never
//! opens them as a document, and neither may whoever shows them.

use base64::Engine as _;
use postrinch_ofscp::ofscp_core::Actor;
use postrinch_ofscp::ofscp_core::wire::{Attachment, Thumbnail};
use postrinch_store::{SocialMedia, social_account_key};
use sha2::{Digest, Sha256};

use super::now_ms;
use crate::{Social, SocialError, SocialResult, parse_actor};

/// How long a cache hit goes without being written down as used: the
/// eviction order need not be finer, and a picture shown often should not
/// write the store each time.
const TOUCH_EVERY_MS: i64 = 60 * 1000;

/// A piece of media to fetch: an attachment, its thumbnail, or an avatar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRef {
    /// The domain of the provider that issued it.
    pub issuer: String,
    /// Its id there (for an avatar, `@` and the user's handle).
    pub id: String,
    pub url: String,
    pub mime: Option<String>,
    /// Its size, as claimed.
    pub size: Option<u64>,
    /// `<algo>:<base64>`, as claimed; `sha-256` is checked.
    pub hash: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    /// A video's poster picture, by URL.
    pub poster: Option<String>,
}

impl MediaRef {
    /// An attachment of a message by `author`, who uploaded it to their
    /// home provider.
    pub fn attachment(author: &Actor, attachment: &Attachment) -> Self {
        Self {
            issuer: author.domain().to_string(),
            id: attachment.id.as_str().to_owned(),
            url: attachment.url.clone(),
            mime: Some(attachment.mime.clone()),
            size: Some(attachment.size),
            hash: attachment.hash.clone(),
            width: attachment.width,
            height: attachment.height,
            duration_ms: attachment.duration_ms,
            poster: attachment.thumbnail.as_ref().map(|t| t.url.clone()),
        }
    }

    /// The thumbnail of an attachment of a message by `author` (stored
    /// with its attachment, by the same provider, §5.8 *Thumbnail*).
    pub fn thumbnail(author: &Actor, thumbnail: &Thumbnail) -> Self {
        Self {
            issuer: author.domain().to_string(),
            id: thumbnail.id.as_str().to_owned(),
            url: thumbnail.url.clone(),
            mime: Some(thumbnail.mime.clone()),
            size: Some(thumbnail.size),
            hash: thumbnail.hash.clone(),
            width: thumbnail.width,
            height: thumbnail.height,
            duration_ms: thumbnail.duration_ms,
            poster: None,
        }
    }

    /// The SHA-256 its `hash` claims, in hex as the cache names blobs;
    /// `None` without one, or with another algorithm.
    fn sha256(&self) -> Option<String> {
        let (algo, digest) = self.hash.as_deref()?.split_once(':')?;
        if !algo.eq_ignore_ascii_case("sha-256") {
            return None;
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(digest)
            .ok()?;
        (bytes.len() == 32).then(|| hex(&bytes))
    }

    /// Its record, keeping its bytes `blob` as fetched at `now`.
    fn record(&self, account: &str, blob: String, size: u64, mime: Option<String>) -> SocialMedia {
        SocialMedia {
            account: account.to_owned(),
            provider: self.issuer.clone(),
            id: self.id.clone(),
            url: self.url.clone(),
            blob: Some(blob),
            fetched: Some(now_ms()),
            used: None,
            mime: self.mime.clone().or(mime),
            size: Some(size),
            width: self.width,
            height: self.height,
            duration_ms: self.duration_ms,
            poster: self.poster.clone(),
        }
    }
}

/// Media's bytes, and what is kept of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedMedia {
    /// Its record, as kept now.
    pub media: SocialMedia,
    pub bytes: Vec<u8>,
    /// Whether the bytes came from the cache, not the network.
    pub from_cache: bool,
}

impl Social {
    /// `media`'s bytes, as `actor`'s account sees it: from the cache when
    /// they are there, else fetched and kept (see the module's docs). A
    /// file larger than the whole cache is not fetched
    /// ([`SocialError::MediaTooLarge`]); one whose bytes do not match its
    /// `hash` is not kept ([`SocialError::MediaDamaged`]). A short-lived
    /// URL past its time answers `403`, `404` or `410`
    /// ([`SocialError::Transport`]): read the message again for a fresh
    /// one (§5.8 *Revocation*).
    pub async fn fetch_media(&self, actor: &str, media: &MediaRef) -> SocialResult<CachedMedia> {
        let account = self.account(&parse_actor(actor)?).await?.actor;
        let _fetching = self.media.fetching.hold(fetch_key(&account, media)).await;
        if let Some(hit) = self.cached(&account, media).await? {
            return Ok(hit);
        }
        self.fetch_into_cache(&account, media, None).await
    }

    /// `media` from the cache, if it is there.
    pub(crate) async fn cached(
        &self,
        account: &str,
        media: &MediaRef,
    ) -> SocialResult<Option<CachedMedia>> {
        let (name, issuer, id) = (account.to_owned(), media.issuer.clone(), media.id.clone());
        let kept = self
            .blocking(move |store| store.social_media(&name, &issuer, &id))
            .await?;
        let claimed = media.sha256();
        if let Some(mut kept) = kept
            && let Some(blob) = kept.blob.clone()
            && claimed.as_ref().is_none_or(|c| *c == blob)
            && let Some(bytes) = self.cached_bytes(&blob).await?
        {
            let now = now_ms();
            let stale = kept
                .used
                .or(kept.fetched)
                .is_none_or(|t| now - t >= TOUCH_EVERY_MS);
            if stale || kept.url != media.url {
                kept.used = Some(now);
                kept.url = media.url.clone();
                let record = kept.clone();
                self.blocking(move |store| store.put_social_media(&record))
                    .await?;
            }
            return Ok(Some(CachedMedia {
                media: kept,
                bytes,
                from_cache: true,
            }));
        }
        // The same bytes, kept for another attachment or account.
        if let Some(blob) = claimed
            && let Some(bytes) = self.cached_bytes(&blob).await?
        {
            let record = media.record(account, blob, bytes.len() as u64, None);
            let kept = record.clone();
            self.blocking(move |store| store.put_social_media(&kept))
                .await?;
            return Ok(Some(CachedMedia {
                media: record,
                bytes,
                from_cache: true,
            }));
        }
        Ok(None)
    }

    /// Fetch `media`, check it, keep it as `record` says (its record made
    /// afresh when `None`), and trim the cache. Called holding its fetch
    /// lock.
    pub(crate) async fn fetch_into_cache(
        &self,
        account: &str,
        media: &MediaRef,
        record: Option<SocialMedia>,
    ) -> SocialResult<CachedMedia> {
        let limit = self.media.options.cache_bytes;
        if let Some(size) = media.size.filter(|s| *s > limit) {
            return Err(SocialError::MediaTooLarge {
                url: media.url.clone(),
                size,
                limit,
            });
        }
        let fetched = self.transport.fetch_media(&media.url, limit).await?;
        let blob = hex(&Sha256::digest(&fetched.bytes));
        if media.sha256().is_some_and(|claimed| claimed != blob) {
            return Err(SocialError::MediaDamaged {
                url: media.url.clone(),
            });
        }
        let bytes = fetched.bytes.to_vec();
        let size = bytes.len() as u64;
        let record = match record {
            Some(mut record) => {
                record.blob = Some(blob.clone());
                record.fetched = Some(now_ms());
                record.size = Some(size);
                record.mime = record.mime.or(fetched.mime);
                record
            }
            None => media.record(account, blob, size, fetched.mime),
        };
        let (kept, to_cache) = (record.clone(), bytes.clone());
        let trimmed = self
            .blocking(move |store| {
                let cached = store.cache_social_media_bytes(&to_cache)?;
                store.put_social_media(&kept)?;
                store.trim_social_media(limit, Some(&cached))
            })
            .await?;
        if trimmed.evicted > 0 {
            tracing::debug!(
                evicted = trimmed.evicted,
                freed = trimmed.freed,
                size = trimmed.size,
                "media cache trimmed"
            );
        }
        Ok(CachedMedia {
            media: record,
            bytes,
            from_cache: false,
        })
    }

    /// The cached bytes `blob`, if they are still there. Bytes the cache
    /// cannot read back whole are evicted, to be fetched again.
    async fn cached_bytes(&self, blob: &str) -> SocialResult<Option<Vec<u8>>> {
        let wanted = blob.to_owned();
        match self
            .blocking(move |store| store.cached_social_media_bytes(&wanted))
            .await
        {
            Ok(bytes) => Ok(bytes),
            Err(SocialError::Store(error)) => {
                tracing::warn!(%blob, %error, "cached media is damaged; it is fetched again");
                let damaged = blob.to_owned();
                self.blocking(move |store| store.evict_social_media(&damaged))
                    .await?;
                Ok(None)
            }
            Err(other) => Err(other),
        }
    }
}

/// The fetch lock's key for `media` of `account`.
pub(crate) fn fetch_key(account: &str, media: &MediaRef) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        social_account_key(account),
        media.issuer.to_ascii_lowercase(),
        media.id
    )
}

/// Lower-case hex, as the blob store names blobs.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_hash(hash: Option<&str>) -> MediaRef {
        MediaRef {
            issuer: "a.example".into(),
            id: "att_1".into(),
            url: "https://a.example/m/1".into(),
            mime: None,
            size: None,
            hash: hash.map(str::to_owned),
            width: None,
            height: None,
            duration_ms: None,
            poster: None,
        }
    }

    #[test]
    fn a_sha256_hash_names_the_blob_its_bytes_are_kept_as() {
        let bytes = b"a picture";
        let claimed = format!(
            "sha-256:{}",
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bytes))
        );
        let blob = hex(&Sha256::digest(bytes));
        assert_eq!(with_hash(Some(&claimed)).sha256(), Some(blob));
        assert_eq!(with_hash(Some("sha-512:AAAA")).sha256(), None);
        assert_eq!(with_hash(Some("sha-256:not base64!")).sha256(), None);
        assert_eq!(with_hash(Some("sha-256:AAAA")).sha256(), None);
        assert_eq!(with_hash(None).sha256(), None);
    }
}
