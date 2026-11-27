//! A queued change's files, uploaded before it (§5.8; docs/ofscp.md,
//! "Outbox": media is uploaded before the message that carries it).
//!
//! Every file goes to the home provider, whatever the message is for,
//! naming as its `target` the provider that will hold the message when
//! that is not home: a remote channel's, or a private message
//! recipient's (§5.8 *Where to upload*, *Target*). Each Attachment is kept
//! in the queued change the moment it comes back
//! ([`postrinch_store::Store::social_change_uploaded`]), so a retry, after
//! a broken connection, a stall or a restart, uploads only what is not up
//! yet. The spec's upload is one request, so a file cut off part-way is
//! sent again from its start.
//!
//! Uploading is not sending: the change is not marked tried, so it can
//! still be taken back, and a failed upload is not counted as an attempt.
//! An upload nothing ever posts is the provider's to drop (§5.8: a pending
//! upload is kept at least 24 hours).

use postrinch_ofscp::http::{MediaUpload, Source};
use postrinch_store::{ContentKind, DraftMedia, SocialChange, SocialOutboxEntry, SocialPlace};

use super::outcome::{Outcome, outcome_of};
use super::send::Sender;
use super::target::Target;
use crate::media::progress::{UploadProgress, Watching};
use crate::{Social, SocialResult};

/// What uploading a change's files came to.
pub(crate) enum Uploaded {
    /// Every file is up: the change as it is queued now.
    Ready(Box<SocialOutboxEntry>),
    /// The change was taken back meanwhile.
    TakenBack,
    /// A file could not be uploaded, now or ever.
    Not(Outcome),
}

impl Social {
    /// Upload `entry`'s files not yet up, in order, to go to `place`.
    pub(crate) async fn upload_files(
        &self,
        sender: &Sender<'_>,
        entry: SocialOutboxEntry,
        place: &SocialPlace,
        target: &Target,
    ) -> SocialResult<Uploaded> {
        let pending = to_upload(sender, place, &entry.change).await?;
        if pending.is_empty() {
            return Ok(Uploaded::Ready(Box::new(entry)));
        }
        let home = sender.home().clone();
        let holder = target.upload_target(&home);
        let client = sender.client(&home);
        let mut entry = entry;
        for (file, media) in pending.iter().enumerate() {
            let upload = match self.upload_of(media).await? {
                Ok(upload) => upload,
                Err(why) => return Ok(Uploaded::Not(Outcome::Refused(why))),
            };
            let upload = match &holder {
                Some(holder) => upload.target(holder.clone()),
                None => upload,
            };
            let watching = Watching::start(
                &self.media.uploads,
                &self.live,
                UploadProgress {
                    account: entry.account.clone(),
                    client_id: entry.client_id.clone(),
                    place: entry.place.clone(),
                    file,
                    files: pending.len(),
                    name: media.file_name().unwrap_or_default(),
                    sent: 0,
                    total: 0,
                },
            );
            let uploaded = client.upload(&upload, Some(watching.callback())).await;
            drop(watching);
            let attachment = match sender.checked(&client, uploaded).await {
                Ok(attachment) => attachment,
                Err(error) => return Ok(Uploaded::Not(not_uploaded(outcome_of(error)?))),
            };
            let attachment = match serde_json::to_value(&attachment) {
                Ok(value) => value,
                Err(e) => {
                    let why = format!("the uploaded attachment does not encode: {e}");
                    return Ok(Uploaded::Not(Outcome::Refused(why)));
                }
            };
            let (account, client_id, media) = (
                entry.account.clone(),
                entry.client_id.clone(),
                media.clone(),
            );
            let now = self
                .blocking(move |store| {
                    store.social_change_uploaded(&account, &client_id, &media, attachment)
                })
                .await?;
            match now {
                Some(now) => entry = now,
                None => return Ok(Uploaded::TakenBack),
            }
        }
        Ok(Uploaded::Ready(Box::new(entry)))
    }

    /// The upload of `media`; why not, when it can never be made (the
    /// file is gone).
    async fn upload_of(&self, media: &DraftMedia) -> SocialResult<Result<MediaUpload, String>> {
        let name = media.file_name().unwrap_or_else(|| "file".to_owned());
        let source = match media {
            DraftMedia::File { path, .. } => Source::File(path.clone()),
            DraftMedia::Kept { blob, .. } => {
                let blob = blob.clone();
                let bytes = self
                    .blocking(move |store| store.social_kept_bytes(&blob))
                    .await?;
                match bytes {
                    Some(bytes) => Source::Bytes(bytes.into()),
                    None => return Ok(Err(format!("{name} is no longer kept here"))),
                }
            }
            DraftMedia::Uploaded { .. } => {
                return Ok(Err(format!("{name} is uploaded already")));
            }
        };
        Ok(Ok(MediaUpload::new(source, name, media.mime())))
    }
}

/// The files `change` carries that are not up yet, each once, in the
/// order they go: an article's cover first, then the attachments. Another
/// kind's cover is not sent, so it is not uploaded.
async fn to_upload(
    sender: &Sender<'_>,
    place: &SocialPlace,
    change: &SocialChange,
) -> SocialResult<Vec<DraftMedia>> {
    let (content, article) = match change {
        SocialChange::Post(post) => (&post.content, post.kind == ContentKind::Article),
        SocialChange::Edit { message, content } => {
            (content, sender.is_article(place, message).await?)
        }
        _ => return Ok(Vec::new()),
    };
    let cover = content.cover.iter().filter(|_| article);
    let mut pending: Vec<DraftMedia> = Vec::new();
    for media in cover.chain(&content.attachments) {
        if !matches!(media, DraftMedia::Uploaded { .. }) && !pending.contains(media) {
            pending.push(media.clone());
        }
    }
    Ok(pending)
}

/// What a failed upload means for its change: it is not sent, so a
/// failure for now is not an attempt.
fn not_uploaded(outcome: Outcome) -> Outcome {
    match outcome {
        Outcome::Again { error, retry_after } => Outcome::NotUploaded { error, retry_after },
        other => other,
    }
}
