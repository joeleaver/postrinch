//! What a queued post or edit says, as the wire carries it (§5.3, §7.1,
//! §7.4).
//!
//! Its files go as the Attachments their uploads gave back. The outbox
//! uploads them first ([`super::upload`]), turning each
//! [`DraftMedia::File`] and [`DraftMedia::Kept`] into
//! [`DraftMedia::Uploaded`]; a file still not uploaded when the change is
//! to go holds it back ([`NotReady::Waiting`]).

use postrinch_ofscp::ofscp_core::wire::{
    Attachment, Content, DmMessageCreate, MessageCreate, MessageReference, MessageType,
    MessageUpdate, Patch,
};
use postrinch_ofscp::ofscp_core::{Actor, LocalId};
use postrinch_store::{ContentKind, DraftMedia, SocialContent, SocialPost};

/// The media type of every body this engine writes: Markdown.
const MARKDOWN: &str = "text/markdown";

/// Why a change cannot be sent as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotReady {
    /// It carries a file not yet uploaded.
    Waiting,
    /// What it keeps of an upload does not read as an Attachment.
    Damaged(String),
}

/// A post's or edit's files: its attachments in order, an article's cover
/// first among them (§5.3: the cover is one of the attachments).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Media {
    pub attachments: Vec<Attachment>,
    pub cover: Option<LocalId>,
}

/// Whether every file `content` carries is uploaded, so it can go: its
/// attachments, and its cover when it is an `article`'s (another kind's
/// cover is not sent, so it holds nothing back).
pub(crate) fn ready(content: &SocialContent, article: bool) -> Result<(), NotReady> {
    media(content, article).map(|_| ())
}

/// `content`'s files as the wire names them; an article's cover is kept,
/// another kind's is not sent.
pub(crate) fn media(content: &SocialContent, article: bool) -> Result<Media, NotReady> {
    let cover = match (&content.cover, article) {
        (Some(cover), true) => Some(attachment(cover)?),
        _ => None,
    };
    let mut attachments: Vec<Attachment> = cover.iter().cloned().collect();
    for media in &content.attachments {
        let attachment = attachment(media)?;
        if !attachments.iter().any(|a| a.id == attachment.id) {
            attachments.push(attachment);
        }
    }
    Ok(Media {
        attachments,
        cover: cover.map(|c| c.id),
    })
}

/// The Attachment an uploaded file is.
fn attachment(media: &DraftMedia) -> Result<Attachment, NotReady> {
    match media {
        DraftMedia::Uploaded { attachment } => serde_json::from_value(attachment.clone())
            .map_err(|e| NotReady::Damaged(format!("a kept attachment does not read: {e}"))),
        DraftMedia::File { .. } | DraftMedia::Kept { .. } => Err(NotReady::Waiting),
    }
}

/// The body as the wire carries it.
fn body(content: &SocialContent) -> Content {
    Content {
        text: content.body.clone(),
        mime: MARKDOWN.to_owned(),
    }
}

/// The wire's name for a kind.
fn message_type(kind: ContentKind) -> MessageType {
    match kind {
        ContentKind::Chat => MessageType::Message,
        ContentKind::Memo => MessageType::Memo,
        ContentKind::Article => MessageType::Article,
    }
}

/// None for an empty list: the wire leaves an absent one out.
fn some_if_any<T>(items: Vec<T>) -> Option<Vec<T>> {
    (!items.is_empty()).then_some(items)
}

/// A post to a channel, sent as `client_id` (§7.1 *Sending messages*),
/// answering `parent`, held by the channel too.
pub(crate) fn channel_post(
    client_id: &str,
    post: &SocialPost,
    parent: Option<MessageReference>,
) -> Result<MessageCreate, NotReady> {
    let article = post.kind == ContentKind::Article;
    let media = media(&post.content, article)?;
    Ok(MessageCreate {
        client_message_id: Some(client_id.to_owned()),
        kind: Some(message_type(post.kind)),
        content: body(&post.content),
        attachments: some_if_any(media.attachments),
        reference: parent,
        tags: some_if_any(post.content.tags.clone()),
        mentions: None,
        title: post.content.title.clone().filter(|_| article),
        summary: None,
        cover: media.cover,
        metadata: None,
    })
}

/// A private message to `to`, sent as `client_id` (§7.4), answering
/// `parent`. A conversation carries chat alone: no title, cover or tags.
pub(crate) fn dm_post(
    client_id: &str,
    to: &Actor,
    post: &SocialPost,
    parent: Option<MessageReference>,
) -> Result<DmMessageCreate, NotReady> {
    let media = media(&post.content, false)?;
    Ok(DmMessageCreate {
        client_message_id: client_id.to_owned(),
        to: Some(to.clone()),
        content: body(&post.content),
        attachments: some_if_any(media.attachments),
        mentions: None,
        reference: parent,
    })
}

/// New content for a message (§7.1 *Editing*): its body and files
/// replaced, and its tags in a channel; an article's title and cover too,
/// each taken away when the edit has none.
pub(crate) fn edit(
    content: &SocialContent,
    article: bool,
    in_channel: bool,
) -> Result<MessageUpdate, NotReady> {
    let media = media(content, article)?;
    let mut update = MessageUpdate {
        content: Some(body(content)),
        attachments: Some(media.attachments),
        tags: in_channel.then(|| content.tags.clone()),
        ..MessageUpdate::default()
    };
    if article {
        update.title = content.title.clone().map_or(Patch::Clear, Patch::Set);
        update.cover = media.cover.map_or(Patch::Clear, Patch::Set);
    }
    Ok(update)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn uploaded(id: &str) -> DraftMedia {
        DraftMedia::Uploaded {
            attachment: json!({
                "id": id,
                "mime": "image/png",
                "url": format!("https://home.example/media/{id}"),
                "size": 3,
            }),
        }
    }

    fn article() -> SocialPost {
        SocialPost {
            kind: ContentKind::Article,
            reply_to: None,
            content: SocialContent {
                body: "# Hello".into(),
                title: Some("Hello".into()),
                tags: vec!["news".into()],
                cover: Some(uploaded("a1")),
                attachments: vec![uploaded("a2"), uploaded("a1")],
            },
        }
    }

    #[test]
    fn an_articles_cover_is_its_first_attachment_and_named() {
        let create = channel_post("c1", &article(), None).unwrap();
        let ids: Vec<&str> = create
            .attachments
            .iter()
            .flatten()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(ids, ["a1", "a2"]);
        assert_eq!(create.cover.unwrap().as_str(), "a1");
        assert_eq!(create.title.as_deref(), Some("Hello"));
        assert_eq!(create.client_message_id.as_deref(), Some("c1"));
        assert_eq!(create.content.mime, "text/markdown");
    }

    #[test]
    fn a_chat_line_sends_no_article_fields() {
        let mut post = article();
        post.kind = ContentKind::Chat;
        let create = channel_post("c1", &post, None).unwrap();
        assert_eq!(create.title, None);
        assert_eq!(create.cover, None);
        let update = edit(&post.content, false, false).unwrap();
        assert!(update.title.is_keep() && update.cover.is_keep());
        assert_eq!(update.tags, None);
    }

    #[test]
    fn an_article_edit_without_a_title_takes_it_away() {
        let mut content = article().content;
        content.title = None;
        content.cover = None;
        let update = edit(&content, true, true).unwrap();
        assert_eq!(update.title, Patch::Clear);
        assert_eq!(update.cover, Patch::Clear);
        assert_eq!(update.tags, Some(vec!["news".to_owned()]));
    }

    #[test]
    fn a_file_not_yet_uploaded_holds_the_change_back() {
        let mut post = article();
        post.content.attachments.push(DraftMedia::File {
            path: PathBuf::from("/tmp/a.png"),
            mime: None,
        });
        assert_eq!(ready(&post.content, true), Err(NotReady::Waiting));
        assert_eq!(
            channel_post("c1", &post, None).unwrap_err(),
            NotReady::Waiting
        );
        post.content.attachments = vec![DraftMedia::Uploaded {
            attachment: json!({"id": 3}),
        }];
        assert!(matches!(
            ready(&post.content, true),
            Err(NotReady::Damaged(_))
        ));
    }

    #[test]
    fn a_cover_not_yet_uploaded_holds_back_only_an_article() {
        let mut post = article();
        post.content.cover = Some(DraftMedia::File {
            path: PathBuf::from("/tmp/cover.png"),
            mime: None,
        });
        assert_eq!(ready(&post.content, true), Err(NotReady::Waiting));
        // Another kind's cover is not sent.
        assert_eq!(ready(&post.content, false), Ok(()));
        post.kind = ContentKind::Chat;
        assert!(channel_post("c1", &post, None).is_ok());
    }
}
