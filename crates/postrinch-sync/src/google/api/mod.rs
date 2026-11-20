//! The parts of the Gmail REST API that sync uses, with retries.

use std::sync::Arc;
use std::time::Duration;

use postrinch_model::EmailAddress;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;

mod labels;
mod outgoing;
mod settings;
mod snippet;
mod types;

#[cfg(test)]
pub(crate) use settings::Criteria;
pub(crate) use settings::FilterResource;
pub(crate) use types::*;

use super::GoogleAuth;
pub(crate) use super::quota::Priority;
use super::quota::{Cost, Quota};

use crate::{SyncError, SyncResult};

const FG: Priority = Priority::Foreground;

const GMAIL: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
/// Attempts per request before a retryable failure is returned.
const ATTEMPTS: u32 = 8;
const MAX_BACKOFF: Duration = Duration::from_secs(32);

/// The Gmail API for any signed-in account.
pub struct GmailApi {
    auth: Arc<GoogleAuth>,
    http: reqwest::Client,
    base: String,
    quota: Quota,
}

impl GmailApi {
    pub fn new(auth: Arc<GoogleAuth>) -> Self {
        Self::with_base(auth, GMAIL.to_owned())
    }

    /// Point at another server: for tests.
    pub fn with_base(auth: Arc<GoogleAuth>, base: String) -> Self {
        Self {
            auth,
            // HTTP/1.1, one connection per concurrent fetch. Over HTTP/2,
            // Gmail sends large raw messages in enough small DATA frames that
            // h2 (0.4.16 on) takes it for a flood and drops the connection
            // ("too_many_data_frames"), and reqwest does not let the limit
            // be raised.
            http: reqwest::Client::builder()
                .gzip(true)
                .http1_only()
                .build()
                .expect("a default HTTP client builds"),
            base,
            quota: Quota::default(),
        }
    }

    pub(crate) async fn profile(&self, account: &EmailAddress) -> SyncResult<Profile> {
        self.get(account, (Cost::GetProfile, FG), "profile", &[])
            .await
    }

    pub(crate) async fn labels(&self, account: &EmailAddress) -> SyncResult<Vec<LabelResource>> {
        Ok(self
            .get::<LabelList>(account, (Cost::ListLabels, FG), "labels", &[])
            .await?
            .labels)
    }

    /// One page of every message id in the account, spam and trash included.
    pub(crate) async fn list_messages(
        &self,
        account: &EmailAddress,
        page: Option<&str>,
    ) -> SyncResult<MessageList> {
        let mut query = vec![("maxResults", "500"), ("includeSpamTrash", "true")];
        if let Some(page) = page {
            query.push(("pageToken", page));
        }
        self.get(account, (Cost::ListMessages, FG), "messages", &query)
            .await
    }

    /// The ids of up to `max` messages Gmail's search finds for `query`,
    /// newest first; one request, so at most 500. As in Gmail, the trash
    /// and spam are searched only when the query names them.
    pub(crate) async fn search(
        &self,
        account: &EmailAddress,
        query: &str,
        max: usize,
    ) -> SyncResult<Vec<String>> {
        let max = max.clamp(1, 500).to_string();
        let list: MessageList = self
            .get(
                account,
                (Cost::ListMessages, FG),
                "messages",
                &[("q", query), ("maxResults", max.as_str())],
            )
            .await?;
        Ok(list.messages.into_iter().map(|m| m.id).collect())
    }

    /// A whole message (60 quota units).
    pub(crate) async fn raw_message(
        &self,
        account: &EmailAddress,
        id: &str,
        priority: Priority,
    ) -> SyncResult<RawMessage> {
        self.get(
            account,
            (Cost::GetMessageRaw, priority),
            &format!("messages/{id}"),
            &[("format", "raw")],
        )
        .await
    }

    /// A message's headers, labels and snippet, without its body (20 quota
    /// units, a third of a whole message).
    pub(crate) async fn metadata_message(
        &self,
        account: &EmailAddress,
        id: &str,
    ) -> SyncResult<MetadataMessage> {
        self.get(
            account,
            (Cost::GetMessageMetadata, FG),
            &format!("messages/{id}"),
            &[("format", "metadata")],
        )
        .await
    }

    /// Where a message is filed, and nothing else (20 quota units).
    pub(crate) async fn minimal_message(
        &self,
        account: &EmailAddress,
        id: &str,
    ) -> SyncResult<MinimalMessage> {
        self.get(
            account,
            (Cost::GetMessageMetadata, FG),
            &format!("messages/{id}"),
            &[("format", "minimal")],
        )
        .await
    }

    /// One page of changes since `start`. Answered 404 when `start` is
    /// older than Gmail keeps history for.
    pub(crate) async fn history(
        &self,
        account: &EmailAddress,
        start: &str,
        page: Option<&str>,
    ) -> SyncResult<HistoryPage> {
        let mut query = vec![
            ("startHistoryId", start),
            ("maxResults", "500"),
            ("historyTypes", "messageAdded"),
            ("historyTypes", "messageDeleted"),
            ("historyTypes", "labelAdded"),
            ("historyTypes", "labelRemoved"),
        ];
        if let Some(page) = page {
            query.push(("pageToken", page));
        }
        self.get(account, (Cost::ListHistory, FG), "history", &query)
            .await
    }

    /// Change messages' labels: add and remove, by Gmail label id (read
    /// and starred are the `UNREAD` and `STARRED` labels). At most 1,000
    /// messages a call.
    pub(crate) async fn batch_modify(
        &self,
        account: &EmailAddress,
        ids: &[String],
        add: &[String],
        remove: &[String],
    ) -> SyncResult<()> {
        let body = serde_json::json!({
            "ids": ids,
            "addLabelIds": add,
            "removeLabelIds": remove,
        });
        self.send(
            account,
            (Cost::BatchModify, FG),
            "messages/batchModify",
            &[],
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// Move a message to the trash.
    pub(crate) async fn trash(&self, account: &EmailAddress, id: &str) -> SyncResult<()> {
        let path = format!("messages/{id}/trash");
        self.send(
            account,
            (Cost::Trash, FG),
            &path,
            &[],
            Some(&serde_json::json!({})),
        )
        .await
        .map(drop)
    }

    /// Take a message out of the trash.
    pub(crate) async fn untrash(&self, account: &EmailAddress, id: &str) -> SyncResult<()> {
        let path = format!("messages/{id}/untrash");
        self.send(
            account,
            (Cost::Trash, FG),
            &path,
            &[],
            Some(&serde_json::json!({})),
        )
        .await
        .map(drop)
    }

    /// GET a JSON answer (see [`Self::send`]).
    async fn get<T: DeserializeOwned>(
        &self,
        account: &EmailAddress,
        cost: (Cost, Priority),
        path: &str,
        query: &[(&str, &str)],
    ) -> SyncResult<T> {
        let body = self.send(account, cost, path, query, None).await?;
        serde_json::from_slice(&body).map_err(|e| {
            SyncError::api("Gmail", 200, format!("unreadable response to {path}: {e}"))
        })
    }

    /// GET, or POST `json`, with retries, paced by the account's quota: a
    /// 401 refreshes the access token once; a rate limit pauses every
    /// request for the account; server errors back off (honouring
    /// `Retry-After`) and try again. Returns the answer's body.
    async fn send(
        &self,
        account: &EmailAddress,
        cost: (Cost, Priority),
        path: &str,
        query: &[(&str, &str)],
        json: Option<&serde_json::Value>,
    ) -> SyncResult<bytes::Bytes> {
        let method = if json.is_some() {
            reqwest::Method::POST
        } else {
            reqwest::Method::GET
        };
        self.request(method, account, cost, path, query, json, true)
            .await
    }

    /// One request, retried on rate limits and server errors (which Gmail
    /// answers without acting), and on dropped connections when `resend`:
    /// not for a call that must not happen twice, such as sending mail,
    /// as a dropped connection may have been after Gmail acted.
    #[allow(clippy::too_many_arguments)]
    async fn request(
        &self,
        method: reqwest::Method,
        account: &EmailAddress,
        cost: (Cost, Priority),
        path: &str,
        query: &[(&str, &str)],
        json: Option<&serde_json::Value>,
        resend: bool,
    ) -> SyncResult<bytes::Bytes> {
        let payload = json.map(Payload::Json);
        self.request_with(method, account, cost, path, query, payload, resend)
            .await
    }

    /// [`Self::request`] with any [`Payload`]: an upload goes to Gmail's
    /// upload address for `path`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn request_with(
        &self,
        method: reqwest::Method,
        account: &EmailAddress,
        (cost, priority): (Cost, Priority),
        path: &str,
        query: &[(&str, &str)],
        payload: Option<Payload<'_>>,
        resend: bool,
    ) -> SyncResult<bytes::Bytes> {
        let mut query = query.to_vec();
        let url = match payload {
            Some(Payload::Upload { .. }) => {
                query.push(("uploadType", "multipart"));
                format!("{}/{path}", upload_base(&self.base))
            }
            _ => format!("{}/{path}", self.base),
        };
        let query = query.as_slice();
        // An upload's body, made once for every attempt.
        let upload = match payload {
            Some(Payload::Upload { metadata, raw }) => Some(related(metadata, raw)),
            _ => None,
        };
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.quota.acquire(&account.key(), cost, priority).await;
            let token = self.auth.access_token(account).await?;
            let mut request = self.http.request(method.clone(), &url);
            match (payload, &upload) {
                (Some(Payload::Json(body)), _) => request = request.json(body),
                (_, Some(body)) => {
                    request = request
                        .header(reqwest::header::CONTENT_TYPE, RELATED_TYPE)
                        .body(body.clone());
                }
                _ => {}
            }
            let response = match request.bearer_auth(&token).query(query).send().await {
                Ok(r) => r,
                Err(e) if resend && attempt < ATTEMPTS && is_transient(&e) => {
                    tracing::debug!("Gmail {path}: {}, retrying", chain(&e));
                    tokio::time::sleep(backoff(attempt, None)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = response.status();
            if status.is_success() {
                // Read the whole body before parsing, so a connection that
                // drops part-way through a large message is retried too.
                let body = match response.bytes().await {
                    Ok(body) => body,
                    Err(e) if resend && attempt < ATTEMPTS && is_transient(&e) => {
                        tracing::debug!("Gmail {path}: {}, retrying", chain(&e));
                        tokio::time::sleep(backoff(attempt, None)).await;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                };
                return Ok(body);
            }
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = response.text().await.unwrap_or_default();
            let (message, reasons) = parse_error(&body);

            if status == StatusCode::UNAUTHORIZED && !refreshed {
                refreshed = true;
                self.auth.invalidate(account).await;
                continue;
            }
            if attempt < ATTEMPTS && is_rate_limit(status, &reasons) {
                tracing::debug!("Gmail {path}: {status} {reasons:?}, pausing the account");
                self.quota
                    .back_off(&account.key(), backoff(attempt, retry_after))
                    .await;
                continue;
            }
            if attempt < ATTEMPTS && is_server_error(status) {
                tracing::debug!("Gmail {path}: {status}, retrying");
                tokio::time::sleep(backoff(attempt, retry_after)).await;
                continue;
            }
            return Err(SyncError::api("Gmail", status.as_u16(), message));
        }
    }
}

/// What a request carries.
#[derive(Clone, Copy)]
pub(crate) enum Payload<'a> {
    Json(&'a serde_json::Value),
    /// A whole message as bytes, beside the JSON that says where it goes:
    /// Gmail's "multipart" upload, for messages too large to send as JSON.
    Upload {
        metadata: &'a serde_json::Value,
        raw: &'a [u8],
    },
}

const BOUNDARY: &str = "postrinch-upload-7f3a9c1e5b";
const RELATED_TYPE: &str = "multipart/related; boundary=postrinch-upload-7f3a9c1e5b";

/// Gmail's upload address for its API at `base`.
fn upload_base(base: &str) -> String {
    match base.find("/gmail/v1") {
        Some(at) => format!("{}/upload{}", &base[..at], &base[at..]),
        None => format!("{base}/upload"),
    }
}

/// A `multipart/related` body: the JSON, then the message.
fn related(metadata: &serde_json::Value, raw: &[u8]) -> bytes::Bytes {
    let mut body = Vec::with_capacity(raw.len() + 512);
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n\
             --{BOUNDARY}\r\nContent-Type: message/rfc822\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(raw);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body.into()
}

#[cfg(test)]
mod upload_tests {
    use super::*;

    #[test]
    fn uploads_go_to_gmail_s_upload_address() {
        assert_eq!(
            upload_base("https://gmail.googleapis.com/gmail/v1/users/me"),
            "https://gmail.googleapis.com/upload/gmail/v1/users/me"
        );
        assert_eq!(
            upload_base("http://127.0.0.1:9/x"),
            "http://127.0.0.1:9/x/upload"
        );
        let body = related(
            &serde_json::json!({"threadId": "t"}),
            b"Subject: hi\r\n\r\nbody",
        );
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.starts_with("--postrinch-upload-7f3a9c1e5b\r\n"));
        assert!(text.contains("{\"threadId\":\"t\"}"));
        assert!(text.contains("message/rfc822\r\n\r\nSubject: hi"));
        assert!(text.ends_with("--postrinch-upload-7f3a9c1e5b--\r\n"));
    }
}

/// Whether a request failed on the way, rather than being refused: a
/// timeout, a connection that would not open or that dropped (HTTP/2
/// connection resets, a server closing an idle connection). Every request
/// here is a GET or a change that only adds, removes or moves to the trash,
/// so sending it again is always safe.
fn is_transient(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_request() || e.is_body()
}

/// An error with its causes, which `reqwest::Error`'s own message leaves out.
pub(crate) fn chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn parse_error(body: &str) -> (String, Vec<String>) {
    match serde_json::from_str::<ErrorBody>(body) {
        Ok(e) => (
            e.error.message,
            e.error.errors.into_iter().map(|r| r.reason).collect(),
        ),
        Err(_) => (body.to_owned(), Vec::new()),
    }
}

/// Gmail saying the account (or the project) is over its quota.
fn is_rate_limit(status: StatusCode, reasons: &[String]) -> bool {
    match status.as_u16() {
        429 => true,
        403 => reasons
            .iter()
            .any(|r| r == "rateLimitExceeded" || r == "userRateLimitExceeded"),
        _ => false,
    }
}

fn is_server_error(status: StatusCode) -> bool {
    matches!(status.as_u16(), 500 | 502 | 503 | 504)
}

/// Exponential backoff from half a second, with up to a quarter of jitter,
/// capped; or what the server asked for.
fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(wait) = retry_after {
        return wait.min(MAX_BACKOFF);
    }
    let base = Duration::from_millis(500)
        .saturating_mul(1 << attempt.min(10))
        .min(MAX_BACKOFF);
    let mut jitter = [0u8; 1];
    rand::fill(&mut jitter[..]);
    base + base.mul_f64(f64::from(jitter[0]) / 1024.0)
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};

    use super::*;

    #[test]
    fn retries_rate_limits_not_permissions() {
        assert!(is_rate_limit(StatusCode::TOO_MANY_REQUESTS, &[]));
        assert!(is_server_error(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_rate_limit(
            StatusCode::FORBIDDEN,
            &["userRateLimitExceeded".into()]
        ));
        assert!(!is_rate_limit(
            StatusCode::FORBIDDEN,
            &["insufficientPermissions".into()]
        ));
        assert!(!is_rate_limit(StatusCode::NOT_FOUND, &[]));
        assert!(!is_server_error(StatusCode::NOT_FOUND));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert!(backoff(1, None) >= Duration::from_secs(1));
        assert!(backoff(1, None) < Duration::from_millis(1300));
        assert!(backoff(20, None) <= MAX_BACKOFF + MAX_BACKOFF / 4);
        assert_eq!(
            backoff(1, Some(Duration::from_secs(3))),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn decodes_raw_padded_or_not() {
        let mut m = RawMessage {
            id: "m".into(),
            thread_id: "t".into(),
            label_ids: vec![],
            snippet: String::new(),
            internal_date: "0".into(),
            raw: URL_SAFE_NO_PAD.encode(b"Subject: hi\r\n\r\n?>"),
        };
        assert_eq!(m.bytes().unwrap(), b"Subject: hi\r\n\r\n?>");
        m.raw = URL_SAFE.encode(b"Subject: hi\r\n\r\n?>");
        assert_eq!(m.bytes().unwrap(), b"Subject: hi\r\n\r\n?>");
    }
}
