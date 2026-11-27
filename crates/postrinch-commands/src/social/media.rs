//! The pictures, videos and files a post or the profile carries, checked
//! before they are queued: each is there to upload, and fits the home
//! provider's limit (§5.8, discovery `capabilities.limits.maxUploadBytes`).
//! Uploading is the engine's.

use postrinch_store::DraftMedia;

use super::{actor_domain, str_at};
use crate::{CommandError, CommandResult, Commands, mime_for};

impl Commands {
    /// An error unless `media` can be uploaded from `actor`'s account: a
    /// file still on this computer, a kept blob still kept, or an
    /// attachment already uploaded; no larger than the home provider takes.
    pub(crate) fn check_upload(&self, actor: &str, media: &DraftMedia) -> CommandResult<()> {
        let size = match media {
            DraftMedia::File { path, .. } => match std::fs::metadata(path) {
                Ok(meta) if meta.is_file() => meta.len(),
                _ => {
                    return Err(CommandError::Invalid(format!(
                        "There is no file {} to send.",
                        path.display()
                    )));
                }
            },
            DraftMedia::Kept { blob, name, .. } => match self.store.social_kept_bytes(blob)? {
                Some(bytes) => bytes.len() as u64,
                None => {
                    return Err(CommandError::Invalid(format!(
                        "{name} is no longer kept here: add it again."
                    )));
                }
            },
            DraftMedia::Uploaded { attachment } => {
                return match str_at(attachment, "id") {
                    Some(_) => Ok(()),
                    None => Err(CommandError::Invalid(
                        "An uploaded attachment has no id.".into(),
                    )),
                };
            }
        };
        if let Some(limit) = self.upload_limit(actor)?
            && size > limit
        {
            return Err(CommandError::Invalid(format!(
                "{} is {}, more than {} takes ({}).",
                media_name(media),
                crate::size_text(size),
                actor_domain(actor),
                crate::size_text(limit)
            )));
        }
        Ok(())
    }

    /// The most the home provider of `actor` takes in one upload, when
    /// discovery said.
    fn upload_limit(&self, actor: &str) -> CommandResult<Option<u64>> {
        let Some(provider) = self.store.social_provider(&actor_domain(actor))? else {
            return Ok(None);
        };
        let limit = |doc: &serde_json::Value| {
            doc.pointer("/limits/maxUploadBytes")
                .or_else(|| doc.pointer("/capabilities/limits/maxUploadBytes"))
                .and_then(serde_json::Value::as_u64)
        };
        Ok(limit(&provider.capabilities).or_else(|| limit(&provider.discovery)))
    }
}

/// What a piece of media is called, for a sentence about it.
fn media_name(media: &DraftMedia) -> String {
    match media {
        DraftMedia::File { path, .. } => crate::file_name(&path.to_string_lossy()),
        DraftMedia::Kept { name, .. } => name.clone(),
        DraftMedia::Uploaded { attachment } => str_at(attachment, "filename")
            .unwrap_or("The attachment")
            .to_owned(),
    }
}

/// Whether `media` is a picture, by its type or else its name.
pub(crate) fn is_picture(media: &DraftMedia) -> bool {
    let mime = match media {
        DraftMedia::File { path, mime } => mime
            .clone()
            .unwrap_or_else(|| mime_for(&path.to_string_lossy()).to_owned()),
        DraftMedia::Kept { name, mime, .. } => {
            mime.clone().unwrap_or_else(|| mime_for(name).to_owned())
        }
        DraftMedia::Uploaded { attachment } => {
            str_at(attachment, "mime").unwrap_or_default().to_owned()
        }
    };
    mime.starts_with("image/")
}
