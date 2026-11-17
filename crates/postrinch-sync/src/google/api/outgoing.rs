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
struct Headers {
    #[serde(default)]
    payload: Payload,
}

#[derive(Default, Deserialize)]
struct Payload {
    #[serde(default)]
    headers: Vec<Header>,
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

impl Headers {
    fn marked(&self, marker: &str) -> bool {
        self.payload.headers.iter().any(|h| {
            h.name.eq_ignore_ascii_case(crate::compose::MARKER_HEADER) && h.value.trim() == marker
        })
    }
}

#[derive(Deserialize)]
struct IdList {
    #[serde(default)]
    messages: Vec<Taken>,
}

/// The JSON Gmail takes for a message, without its bytes: for a reply,
/// the thread to file it in.
fn message_json(thread: Option<&str>) -> serde_json::Value {
    let mut message = json!({});
    if let Some(thread) = thread {
        message["threadId"] = json!(thread);
    }
    message
}

/// Messages larger than this go up as uploads: Gmail takes about 5 MB of
/// JSON, and base64 makes a message a third larger.
const JSON_MAX_RAW: usize = 3_500_000;

fn read<T: for<'de> Deserialize<'de>>(body: &[u8], what: &str) -> SyncResult<T> {
    serde_json::from_slice(body)
        .map_err(|e| SyncError::api("Gmail", 200, format!("unreadable response to {what}: {e}")))
}

impl GmailApi {
    /// Give Gmail a message: `metadata` with its bytes inside (under
    /// `message` when `in_message`), or, for a large message, as an
    /// upload beside it. Never retried on a dropped connection.
    #[allow(clippy::too_many_arguments)]
    async fn post_message(
        &self,
        method: Method,
        account: &EmailAddress,
        cost: (Cost, super::Priority),
        path: &str,
        mut metadata: serde_json::Value,
        in_message: bool,
        raw: &[u8],
    ) -> SyncResult<bytes::Bytes> {
        if raw.len() > JSON_MAX_RAW {
            let payload = super::Payload::Upload {
                metadata: &metadata,
                raw,
            };
            return self
                .request_with(method, account, cost, path, &[], Some(payload), false)
                .await;
        }
        let encoded = json!(URL_SAFE_NO_PAD.encode(raw));
        if in_message {
            metadata["message"]["raw"] = encoded;
        } else {
            metadata["raw"] = encoded;
        }
        self.request(method, account, cost, path, &[], Some(&metadata), false)
            .await
    }

    /// Send a message. Never sent twice by a retry here: a connection that
    /// drops is an error, and [`GmailApi::find_sent`] tells whether Gmail
    /// took it before the next attempt.
    pub(crate) async fn send_message(
        &self,
        account: &EmailAddress,
        raw: &[u8],
        thread: Option<&str>,
    ) -> SyncResult<Taken> {
        let answer = self
            .post_message(
                Method::POST,
                account,
                (Cost::SendMessage, FG),
                "messages/send",
                message_json(thread),
                false,
                raw,
            )
            .await?;
        read(&answer, "messages/send")
    }

    /// The sent message marked `marker` (see `compose::MARKER_HEADER`),
    /// among those sent since `since` (Unix seconds): a send that may or
    /// may not have gone through. Gmail rewrites `Message-ID`, so the
    /// marker is read from each recent sent message.
    pub(crate) async fn find_sent(
        &self,
        account: &EmailAddress,
        marker: &str,
        since: i64,
    ) -> SyncResult<Option<Taken>> {
        let q = format!("in:sent after:{since}");
        let list: IdList = self
            .get(
                account,
                (Cost::ListMessages, FG),
                "messages",
                &[("q", q.as_str()), ("maxResults", "50")],
            )
            .await?;
        for message in list.messages {
            let path = format!("messages/{}", message.id);
            let found: Headers = self
                .get(
                    account,
                    (Cost::GetMessageMetadata, FG),
                    &path,
                    &[
                        ("format", "metadata"),
                        ("metadataHeaders", crate::compose::MARKER_HEADER),
                    ],
                )
                .await?;
            if found.marked(marker) {
                return Ok(Some(message));
            }
        }
        Ok(None)
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
        let body = json!({ "message": message_json(thread) });
        let existing = match replacing {
            Some(message) => self.draft_of(account, message).await?,
            None => None,
        };
        let answer = match existing {
            Some(draft) => {
                let path = format!("drafts/{draft}");
                let mut body = body;
                body["id"] = json!(draft);
                self.post_message(
                    Method::PUT,
                    account,
                    (Cost::Draft, FG),
                    &path,
                    body,
                    true,
                    raw,
                )
                .await?
            }
            None => {
                self.post_message(
                    Method::POST,
                    account,
                    (Cost::Draft, FG),
                    "drafts",
                    body,
                    true,
                    raw,
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
