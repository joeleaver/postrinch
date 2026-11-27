//! Media, both ways (the plan's PR 11): a queued change's files uploaded
//! before it, and the media messages carry, and avatars, fetched into a
//! bounded cache and served from it again.

mod avatar;
mod cache;
mod uploads;

use std::path::{Path, PathBuf};

use postrinch_ofscp::http::{Client, MediaUpload, Source};
use postrinch_ofscp::ofscp_core::wire::Attachment;
use postrinch_ofscp_fake::FakeProvider;
use postrinch_store::{DraftMedia, SocialChange};

use crate::outbox::chat;

/// `len` bytes that say `seed`, so two files of one length differ.
pub fn bytes(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| seed.wrapping_add((i % 251) as u8))
        .collect()
}

/// A file `name` in `dir` holding `content`.
pub fn file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// A chat line saying `body`, carrying `files`.
pub fn with_files(body: &str, files: Vec<DraftMedia>) -> SocialChange {
    let mut post = chat(body, None);
    if let SocialChange::Post(p) = &mut post {
        p.content.attachments = files;
    }
    post
}

/// The file at `path`, to attach.
pub fn attached(path: &Path, mime: &str) -> DraftMedia {
    DraftMedia::File {
        path: path.to_owned(),
        mime: Some(mime.into()),
    }
}

/// `content` uploaded by whoever `client` signs as, as `name`.
pub async fn uploaded(client: &Client, name: &str, content: Vec<u8>) -> Attachment {
    let upload = MediaUpload::new(Source::Bytes(content.into()), name, "image/png");
    client.upload(&upload, None).await.expect("uploaded")
}

/// How many times media was fetched from `provider` (its unsigned
/// capability URLs).
pub fn media_fetches(provider: &FakeProvider) -> usize {
    provider
        .requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.starts_with("/media/"))
        .count()
}

/// How many uploads `provider` answered (one whose connection was cut
/// part-way is not answered, so not counted).
pub fn uploads(provider: &FakeProvider) -> usize {
    provider
        .requests()
        .iter()
        .filter(|r| r.method == "POST" && r.path == "/api/media")
        .count()
}
