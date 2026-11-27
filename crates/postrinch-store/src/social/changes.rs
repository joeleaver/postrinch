//! What a change made here says, as it waits in the outbox for its
//! provider ([`super::SocialOutboxEntry::change`]). The commands make
//! them (`postrinch-commands`' `social/`), the engine sends them, and
//! reads lay the pending ones over what sync kept, so a change shows at
//! once and sync never undoes one.
//!
//! A change of one place (a post, an edit, a read marker, a follow) names
//! it in the entry's `place`; the change itself names only the rest.

use serde::{Deserialize, Serialize};

use super::{ContentKind, DraftMedia, Notices};

/// A change on its way to a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum SocialChange {
    /// A new message in the place: a chat line, memo or article, or a
    /// reply to one (§7.1 `POST …/messages`, spec PR lost-conn/ofscp#23).
    /// The entry's client id goes with it as its `clientMessageId`.
    Post(SocialPost),
    /// New content for a message of the account's own in the place (§7.1
    /// *Editing*, spec PR lost-conn/ofscp#24).
    Edit {
        message: String,
        content: SocialContent,
    },
    /// Delete a message in the place, leaving its tombstone (§7.1).
    Delete { message: String },
    /// Add (`on`) or take back one of the account's reactions to a message
    /// in the place (§7.1).
    React {
        message: String,
        key: String,
        on: bool,
    },
    /// Move the place's read marker to a message (§7.7). Only a channel
    /// the home provider holds has one; elsewhere read state is kept here
    /// alone, and no such change is made.
    Read { message: String },
    /// Follow the channel, or stop following it (§7.6). `group` is the
    /// channel's group URI, which the follow names when it is known.
    Follow { group: Option<String>, on: bool },
    /// How the channel tells the person of new messages (§10.2); `None`
    /// for its group's setting.
    ChannelNotices { notices: Option<Notices> },
    /// How a group's channels tell the person of new messages (§10.2);
    /// `None` for the provider's default. A change of no place.
    GroupNotices {
        group: String,
        notices: Option<Notices>,
    },
    /// The account's profile (§6.3). A change of no place.
    Profile(ProfileChange),
}

/// A message being posted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialPost {
    pub kind: ContentKind,
    /// The message it answers, in the same place: always one that answers
    /// none, as replies go one level deep (§7.2).
    pub reply_to: Option<String>,
    #[serde(flatten)]
    pub content: SocialContent,
}

/// What a message says: what a post is made with, and what an edit
/// replaces.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SocialContent {
    /// The body, in Markdown (sent as `text/markdown`).
    pub body: String,
    /// An article's title (§5.3 *Article fields*); `None` for other kinds,
    /// and for an article whose body's first heading is its title.
    pub title: Option<String>,
    pub tags: Vec<String>,
    /// An article's cover picture.
    pub cover: Option<DraftMedia>,
    /// The pictures, videos and files it carries, in order. Those not yet
    /// uploaded are uploaded first, to the home provider.
    pub attachments: Vec<DraftMedia>,
}

/// A change to the account's profile: each field `None` is left as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileChange {
    pub display_name: Option<String>,
    /// The about line.
    pub bio: Option<String>,
    pub avatar: Option<AvatarChange>,
}

/// A new profile picture, or none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "avatar")]
pub enum AvatarChange {
    /// This picture, uploaded first if it is not yet.
    Set { media: DraftMedia },
    /// No picture.
    Remove,
}

impl SocialChange {
    /// The blob store's blobs it uploads first (a pasted picture): kept
    /// while it is queued.
    pub fn blobs(&self) -> Vec<String> {
        let media: Vec<&DraftMedia> = match self {
            Self::Post(post) => post.content.media().collect(),
            Self::Edit { content, .. } => content.media().collect(),
            Self::Profile(ProfileChange {
                avatar: Some(AvatarChange::Set { media }),
                ..
            }) => vec![media],
            _ => Vec::new(),
        };
        media
            .into_iter()
            .filter_map(|m| match m {
                DraftMedia::Kept { blob, .. } => Some(blob.clone()),
                DraftMedia::File { .. } | DraftMedia::Uploaded { .. } => None,
            })
            .collect()
    }
}

impl SocialChange {
    /// Put `uploaded` wherever `media` is among the files the change
    /// carries (a post's or an edit's cover and attachments, a profile's
    /// avatar), once `media` is uploaded. Returns whether it was there.
    pub fn replace_media(&mut self, media: &DraftMedia, uploaded: &DraftMedia) -> bool {
        let slots: Vec<&mut DraftMedia> = match self {
            Self::Post(post) => post.content.media_mut().collect(),
            Self::Edit { content, .. } => content.media_mut().collect(),
            Self::Profile(ProfileChange {
                avatar: Some(AvatarChange::Set { media }),
                ..
            }) => vec![media],
            _ => Vec::new(),
        };
        let mut found = false;
        for slot in slots {
            if slot == media {
                *slot = uploaded.clone();
                found = true;
            }
        }
        found
    }
}

impl SocialContent {
    /// Its cover, then its attachments.
    pub fn media(&self) -> impl Iterator<Item = &DraftMedia> {
        self.cover.iter().chain(&self.attachments)
    }

    fn media_mut(&mut self) -> impl Iterator<Item = &mut DraftMedia> {
        self.cover.iter_mut().chain(&mut self.attachments)
    }
}

impl DraftMedia {
    /// The name it is uploaded under: the file's, or the kept picture's;
    /// an uploaded one's, if it has one.
    pub fn file_name(&self) -> Option<String> {
        match self {
            Self::File { path, .. } => path.file_name().map(|n| n.to_string_lossy().into_owned()),
            Self::Kept { name, .. } => Some(name.clone()),
            Self::Uploaded { attachment } => attachment
                .get("filename")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        }
    }

    /// Its media type: as given, else as its name suggests.
    pub fn mime(&self) -> String {
        let given = match self {
            Self::File { mime, .. } | Self::Kept { mime, .. } => mime.clone(),
            Self::Uploaded { attachment } => attachment
                .get("mime")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        };
        given.unwrap_or_else(|| {
            postrinch_model::mime_for(&self.file_name().unwrap_or_default()).to_owned()
        })
    }
}
