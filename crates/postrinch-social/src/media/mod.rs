//! Media, both ways (the plan's PR 11; §5.8, §6.3).
//!
//! - **Uploads** (the outbox's `upload`): the outbox uploads a change's files before
//!   the change, to the home provider, naming the provider that will hold
//!   the message as the upload's `target`. Each file's Attachment is kept
//!   in the queued change as soon as its provider gives it, so a retry
//!   (after a broken connection, a stall, a restart) uploads only the
//!   files not yet up; the spec's single-step upload cannot resume a file
//!   part-way, so an interrupted one starts again. Progress is told live
//!   and kept while it runs ([`progress`]).
//! - **Fetching** ([`fetch`]): a picture, video or file a message carries
//!   is fetched unsigned by its capability URL, checked against its
//!   `hash`, and kept in the store's media cache; asked for again, it is
//!   served from there. The cache is kept to [`MediaOptions::cache_bytes`],
//!   evicting what was used longest ago.
//! - **Avatars** ([`avatar`]): a user's avatar is read from their profile
//!   and cached the same way, read again once it is older than
//!   [`MediaOptions::avatar_ttl`].

pub(crate) mod avatar;
pub(crate) mod fetch;
pub(crate) mod progress;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Social;
use crate::locks::PassingLocks;

pub use fetch::{CachedMedia, MediaRef};
pub use progress::UploadProgress;

/// How media is cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaOptions {
    /// The most the media cache holds, in bytes. A file larger than this
    /// is not fetched at all ([`crate::SocialError::MediaTooLarge`]).
    pub cache_bytes: u64,
    /// How long an avatar is served from the cache before the user's
    /// profile is read again, to see whether it changed.
    pub avatar_ttl: Duration,
}

impl Default for MediaOptions {
    fn default() -> Self {
        Self {
            cache_bytes: 1024 * 1024 * 1024,
            avatar_ttl: Duration::from_secs(24 * 60 * 60),
        }
    }
}

/// The media engine's state, shared by the engine's clones.
#[derive(Default)]
pub(crate) struct Media {
    pub(crate) options: MediaOptions,
    pub(crate) uploads: Arc<progress::Uploads>,
    /// One fetch of each piece of media at a time, so a second asking
    /// waits and is served from the cache.
    pub(crate) fetching: PassingLocks,
}

impl Social {
    /// The engine, caching media as `options` say.
    pub fn with_media_options(mut self, options: MediaOptions) -> Self {
        self.media = Arc::new(Media {
            options,
            ..Media::default()
        });
        self
    }

    /// How media is cached.
    pub fn media_options(&self) -> &MediaOptions {
        &self.media.options
    }
}

/// Now, in milliseconds since the epoch, as the store keeps times.
pub(crate) fn now_ms() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
}
