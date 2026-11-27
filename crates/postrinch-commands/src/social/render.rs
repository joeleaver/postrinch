//! A message's body ready to show, cleaned as mail is (`postrinch-render`'s
//! `social`), its pictures from the blob store once fetched.

use base64::Engine as _;
use postrinch_render::social::{Attachment, Body, Options, Rendered};
use postrinch_store::{PlaceKind, SocialMessage, social_account_key};
use serde_json::Value;

use super::{actor_domain, str_at, uri_domain};
use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// `message`'s body (as a page or thread gives it) ready to show, with
    /// pictures from their addresses only when `pictures` (§5.3: once the
    /// person asks, or a setting of theirs says so). A picture already
    /// fetched is shown either way.
    pub fn render_social(
        &self,
        message: &SocialMessage,
        pictures: bool,
    ) -> CommandResult<Rendered> {
        let account = self.check_social(&message.account)?;
        let Some(place) = self.store.social_place(&account.actor, &message.place)? else {
            return Err(CommandError::Invalid(format!(
                "{} knows no place {}.",
                account.actor, message.place
            )));
        };
        let object = &message.message;
        let listed = object.get("attachments").and_then(Value::as_array);
        let mut kept: Vec<(String, String, Option<String>, Option<String>)> = Vec::new();
        for attachment in listed.into_iter().flatten() {
            let Some(id) = str_at(attachment, "id") else {
                continue;
            };
            let mime = str_at(attachment, "mime").unwrap_or_default().to_owned();
            let picture = self.picture(&account.actor, attachment, &mime)?;
            let name = str_at(attachment, "filename").map(str::to_owned);
            kept.push((id.to_owned(), mime, name, picture));
        }
        let attachments: Vec<Attachment> = kept
            .iter()
            .map(|(id, mime, name, picture)| Attachment {
                id,
                mime,
                name: name.as_deref(),
                picture: picture.as_deref(),
            })
            .collect();
        let mentions: Option<Vec<String>> =
            object.get("mentions").and_then(Value::as_array).map(|m| {
                m.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
        // Whom a bare @handle names: the channel's provider, or in a
        // conversation (held on both sides) the author's own.
        let domain = match place.kind {
            PlaceKind::Channel => {
                uri_domain(&place.id).unwrap_or_else(|| actor_domain(&account.actor))
            }
            PlaceKind::Conversation => actor_domain(&message.author),
        };
        let content = object.get("content").unwrap_or(&Value::Null);
        let body = Body {
            mime: str_at(content, "mime").unwrap_or("text/plain"),
            text: str_at(content, "text").unwrap_or_default(),
            attachments: &attachments,
            mentions: mentions.as_deref(),
            domain: &domain,
        };
        Ok(postrinch_render::social::render(
            &body,
            Options { pictures },
        ))
    }

    /// Where an attachment's picture can be loaded from: a `data:` URL of
    /// the copy fetched into the blob store, or else its own address. `None`
    /// for what is not a picture.
    fn picture(
        &self,
        actor: &str,
        attachment: &Value,
        mime: &str,
    ) -> CommandResult<Option<String>> {
        if !mime.starts_with("image/") {
            return Ok(None);
        }
        let Some(url) = str_at(attachment, "url") else {
            return Ok(None);
        };
        let key = social_account_key(actor);
        let fetched = self
            .store
            .social_media_at(url)?
            .into_iter()
            .filter(|m| social_account_key(&m.account) == key)
            .find_map(|m| m.blob.map(|blob| (blob, m.mime)));
        if let Some((blob, kept_mime)) = fetched
            && let Some(bytes) = self.store.social_media_bytes(&blob)?
        {
            let mime = kept_mime.as_deref().unwrap_or(mime);
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            return Ok(Some(format!("data:{mime};base64,{data}")));
        }
        Ok(Some(url.to_owned()))
    }
}
