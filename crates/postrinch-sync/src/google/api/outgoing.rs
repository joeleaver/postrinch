//! Gmail's calls for mail written here: sending, and keeping drafts.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use postrinch_model::EmailAddress;
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use super::{FG, GmailApi};
use crate::google::quota::Cost;
use crate::{SyncError, SyncResult};

/// A message Gmail took.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Taken {
    pub id: String,
}

#[derive(Deserialize)]
struct DraftResource {
    id: String,
    message: Taken,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftList {
    #[serde(default)]
    drafts: Vec<DraftResource>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct IdList {
    #[serde(default)]
    messages: Vec<Taken>,
}

/// The JSON Gmail takes for a message: its raw bytes and, for a reply, the
/// thread to file it in.
fn message_json(raw: &[u8], thread: Option<&str>) -> serde_json::Value {
    let mut message = json!({ "raw": URL_SAFE_NO_PAD.encode(raw) });
    if let Some(thread) = thread {
        message["threadId"] = json!(thread);
    }
    message
}

fn read<T: for<'de> Deserialize<'de>>(body: &[u8], what: &str) -> SyncResult<T> {
    serde_json::from_slice(body)
        .map_err(|e| SyncError::api("Gmail", 200, format!("unreadable response to {what}: {e}")))
}

impl GmailApi {
    /// Send a message. Never sent twice by a retry here: a connection that
    /// drops is an error, and [`GmailApi::find_by_message_id`] tells
    /// whether Gmail took it before the next attempt.
    pub(crate) async fn send_message(
        &self,
        account: &EmailAddress,
        raw: &[u8],
        thread: Option<&str>,
    ) -> SyncResult<Taken> {
        let body = message_json(raw, thread);
        let answer = self
            .request(
                Method::POST,
                account,
                (Cost::SendMessage, FG),
                "messages/send",
                &[],
                Some(&body),
                false,
            )
            .await?;
        read(&answer, "messages/send")
    }

    /// The message with this `Message-ID` header (without brackets), if
    /// Gmail has one: a send that may or may not have gone through.
    pub(crate) async fn find_by_message_id(
        &self,
        account: &EmailAddress,
        message_id: &str,
    ) -> SyncResult<Option<Taken>> {
        let q = format!("rfc822msgid:{message_id}");
        let list: IdList = self
            .get(
                account,
                (Cost::ListMessages, FG),
                "messages",
                &[("q", q.as_str()), ("includeSpamTrash", "true")],
            )
            .await?;
        Ok(list.messages.into_iter().next())
    }

    /// Keep a message as a draft: a new one, or in place of the draft whose
    /// message is `replacing`. Returns the draft's message.
    pub(crate) async fn keep_draft(
        &self,
        account: &EmailAddress,
        raw: &[u8],
        thread: Option<&str>,
        replacing: Option<&str>,
    ) -> SyncResult<Taken> {
        let body = json!({ "message": message_json(raw, thread) });
        let existing = match replacing {
            Some(message) => self.draft_of(account, message).await?,
            None => None,
        };
        let answer = match existing {
            Some(draft) => {
                let path = format!("drafts/{draft}");
                let mut body = body;
                body["id"] = json!(draft);
                self.request(
                    Method::PUT,
                    account,
                    (Cost::Draft, FG),
                    &path,
                    &[],
                    Some(&body),
                    false,
                )
                .await?
            }
            None => {
                self.request(
                    Method::POST,
                    account,
                    (Cost::Draft, FG),
                    "drafts",
                    &[],
                    Some(&body),
                    false,
                )
                .await?
            }
        };
        Ok(read::<DraftResource>(&answer, "drafts")?.message)
    }

    /// Delete the draft whose message is `message`, if Gmail still has it:
    /// after the message it became was sent.
    pub(crate) async fn delete_draft(
        &self,
        account: &EmailAddress,
        message: &str,
    ) -> SyncResult<()> {
        let Some(draft) = self.draft_of(account, message).await? else {
            return Ok(());
        };
        let path = format!("drafts/{draft}");
        self.request(
            Method::DELETE,
            account,
            (Cost::Draft, FG),
            &path,
            &[],
            None,
            true,
        )
        .await
        .map(drop)
    }

    /// The id of the draft whose message is `message`.
    async fn draft_of(&self, account: &EmailAddress, message: &str) -> SyncResult<Option<String>> {
        let mut page: Option<String> = None;
        loop {
            let mut query = vec![("maxResults", "500")];
            if let Some(token) = &page {
                query.push(("pageToken", token.as_str()));
            }
            let list: DraftList = self
                .get(account, (Cost::ListMessages, FG), "drafts", &query)
                .await?;
            if let Some(found) = list.drafts.into_iter().find(|d| d.message.id == message) {
                return Ok(Some(found.id));
            }
            match list.next_page_token {
                Some(token) => page = Some(token),
                None => return Ok(None),
            }
        }
    }
}
