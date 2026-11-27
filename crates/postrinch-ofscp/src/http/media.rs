//! Uploading media (§5.8, spec #24): `POST /api/media`, one multipart
//! request, always to the uploader's home provider.

use ofscp_core::Authority;
use ofscp_core::wire::Attachment;
use reqwest::Method;

use super::Client;
use super::multipart::{Part, ProgressFn, Source, Upload};
use super::send::Call;
use crate::Result;

/// A file to upload, with what the provider may not work out for itself.
#[derive(Debug, Clone)]
pub struct MediaUpload {
    source: Source,
    filename: String,
    mime: String,
    target: Option<Authority>,
    thumbnail: Option<(Source, String, String)>,
    width: Option<u32>,
    height: Option<u32>,
    duration_ms: Option<u64>,
}

impl MediaUpload {
    /// `source`, sent as `filename` with type `mime`. The provider sets
    /// the stored `mime`, `size` and `hash` from the bytes.
    pub fn new(source: Source, filename: impl Into<String>, mime: impl Into<String>) -> Self {
        Self {
            source,
            filename: filename.into(),
            mime: mime.into(),
            target: None,
            thumbnail: None,
            width: None,
            height: None,
            duration_ms: None,
        }
    }

    /// The provider that will hold the messages using the upload, when it
    /// is not the home provider: a remote channel's, or a private message
    /// recipient's (§5.8 "Target"). To use the same file on another
    /// provider, upload it again with that one as the target.
    pub fn target(mut self, target: Authority) -> Self {
        self.target = Some(target);
        self
    }

    /// A preview image, such as a video's poster frame (§5.8 "Thumbnail").
    pub fn thumbnail(
        mut self,
        source: Source,
        filename: impl Into<String>,
        mime: impl Into<String>,
    ) -> Self {
        self.thumbnail = Some((source, filename.into(), mime.into()));
        self
    }

    /// Pixel dimensions, for when the provider cannot read them (§5.8).
    pub fn dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    /// Duration of audio or video, for when the provider cannot read it.
    pub fn duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    fn parts(&self) -> Vec<Part> {
        let mut parts = vec![Part::File {
            name: "file",
            filename: self.filename.clone(),
            mime: self.mime.clone(),
            source: self.source.clone(),
        }];
        if let Some((source, filename, mime)) = &self.thumbnail {
            parts.push(Part::File {
                name: "thumbnail",
                filename: filename.clone(),
                mime: mime.clone(),
                source: source.clone(),
            });
        }
        let text = |name: &'static str, value: String| Part::Text { name, value };
        parts.extend(self.target.as_ref().map(|t| text("target", t.to_string())));
        parts.extend(self.width.map(|w| text("width", w.to_string())));
        parts.extend(self.height.map(|h| text("height", h.to_string())));
        parts.extend(self.duration_ms.map(|d| text("durationMs", d.to_string())));
        parts
    }
}

impl Client {
    /// Upload `media` (§5.8) and get back the hosted [`Attachment`], to
    /// put unchanged in a message's `attachments` or a profile's `avatar`.
    /// Only the signer's home provider takes uploads; `progress` hears how
    /// far the body has got. A file over the provider's limit is a `413`
    /// (`capabilities.limits.maxUploadBytes` in discovery says the limit).
    pub async fn upload(
        &self,
        media: &MediaUpload,
        progress: Option<ProgressFn>,
    ) -> Result<Attachment> {
        self.require_home()?;
        let upload = Upload::prepare(media.parts(), progress).await?;
        let call = Call::new(Method::POST, "/api/media".to_owned()).upload(&upload);
        self.provider()
            .execute(super::send::Auth::Signed(self.identity()), call)
            .await?
            .json()
    }
}
