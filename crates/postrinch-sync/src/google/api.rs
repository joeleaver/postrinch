//! The parts of the Gmail REST API that sync uses, with retries.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use postrinch_model::EmailAddress;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::GoogleAuth;
use crate::{SyncError, SyncResult};

const GMAIL: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
/// Attempts per request before a retryable failure is returned.
const ATTEMPTS: u32 = 6;
const MAX_BACKOFF: Duration = Duration::from_secs(32);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Profile {
    pub history_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LabelResource {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Deserialize)]
struct LabelList {
    #[serde(default)]
    labels: Vec<LabelResource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageRef {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MessageList {
    #[serde(default)]
    pub messages: Vec<MessageRef>,
    pub next_page_token: Option<String>,
}

/// A message as `format=raw` returns it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawMessage {
    pub id: String,
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub snippet: String,
    /// Milliseconds since the epoch, as a string.
    pub internal_date: String,
    pub raw: String,
}

impl RawMessage {
    /// The message's bytes. Gmail documents base64url; accept it padded or
    /// not.
    pub(crate) fn bytes(&self) -> SyncResult<Vec<u8>> {
        URL_SAFE_NO_PAD
            .decode(self.raw.trim_end_matches('='))
            .or_else(|_| URL_SAFE.decode(&self.raw))
            .map_err(|e| SyncError::Mime(format!("message {}: raw is not base64url: {e}", self.id)))
    }
}

/// A message as `format=minimal` returns it: just where it is filed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MinimalMessage {
    pub id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryMessage {
    pub message: MessageRef,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryRecord {
    #[serde(default)]
    pub messages_added: Vec<HistoryMessage>,
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessage>,
    #[serde(default)]
    pub labels_added: Vec<HistoryMessage>,
    #[serde(default)]
    pub labels_removed: Vec<HistoryMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryPage {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    pub next_page_token: Option<String>,
    pub history_id: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    message: String,
    #[serde(default)]
    errors: Vec<ErrorReason>,
}

#[derive(Deserialize)]
struct ErrorReason {
    #[serde(default)]
    reason: String,
}

/// The Gmail API for any signed-in account.
pub struct GmailApi {
    auth: Arc<GoogleAuth>,
    http: reqwest::Client,
    base: String,
}

impl GmailApi {
    pub fn new(auth: Arc<GoogleAuth>) -> Self {
        Self::with_base(auth, GMAIL.to_owned())
    }

    /// Point at another server: for tests.
    pub fn with_base(auth: Arc<GoogleAuth>, base: String) -> Self {
        Self {
            auth,
            http: reqwest::Client::builder()
                .gzip(true)
                .build()
                .expect("a default HTTP client builds"),
            base,
        }
    }

    pub(crate) async fn profile(&self, account: &EmailAddress) -> SyncResult<Profile> {
        self.get(account, "profile", &[]).await
    }

    pub(crate) async fn labels(&self, account: &EmailAddress) -> SyncResult<Vec<LabelResource>> {
        Ok(self.get::<LabelList>(account, "labels", &[]).await?.labels)
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
        self.get(account, "messages", &query).await
    }

    pub(crate) async fn raw_message(
        &self,
        account: &EmailAddress,
        id: &str,
    ) -> SyncResult<RawMessage> {
        self.get(account, &format!("messages/{id}"), &[("format", "raw")])
            .await
    }

    pub(crate) async fn minimal_message(
        &self,
        account: &EmailAddress,
        id: &str,
    ) -> SyncResult<MinimalMessage> {
        self.get(account, &format!("messages/{id}"), &[("format", "minimal")])
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
        self.get(account, "history", &query).await
    }

    /// GET with retries: a 401 refreshes the access token once; rate limits
    /// and server errors back off (honouring `Retry-After`) and try again.
    async fn get<T: DeserializeOwned>(
        &self,
        account: &EmailAddress,
        path: &str,
        query: &[(&str, &str)],
    ) -> SyncResult<T> {
        let url = format!("{}/{path}", self.base);
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let token = self.auth.access_token(account).await?;
            let response = match self
                .http
                .get(&url)
                .bearer_auth(&token)
                .query(query)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) if attempt < ATTEMPTS && is_transient(&e) => {
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
                    Err(e) if attempt < ATTEMPTS && is_transient(&e) => {
                        tracing::debug!("Gmail {path}: {}, retrying", chain(&e));
                        tokio::time::sleep(backoff(attempt, None)).await;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                };
                return serde_json::from_slice(&body).map_err(|e| {
                    SyncError::api(
                        "Gmail",
                        status.as_u16(),
                        format!("unreadable response to {path}: {e}"),
                    )
                });
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
            if attempt < ATTEMPTS && is_retryable(status, &reasons) {
                tracing::debug!("Gmail {path}: {status}, retrying");
                tokio::time::sleep(backoff(attempt, retry_after)).await;
                continue;
            }
            return Err(SyncError::api("Gmail", status.as_u16(), message));
        }
    }
}

/// Whether a request failed on the way, rather than being refused: a
/// timeout, a connection that would not open or that dropped (HTTP/2
/// connection resets, a server closing an idle connection). Every request
/// here is a GET, so trying again is always safe.
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

fn is_retryable(status: StatusCode, reasons: &[String]) -> bool {
    match status.as_u16() {
        429 | 500 | 502 | 503 | 504 => true,
        403 => reasons
            .iter()
            .any(|r| r == "rateLimitExceeded" || r == "userRateLimitExceeded"),
        _ => false,
    }
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
    use super::*;

    #[test]
    fn retries_rate_limits_not_permissions() {
        assert!(is_retryable(StatusCode::TOO_MANY_REQUESTS, &[]));
        assert!(is_retryable(StatusCode::SERVICE_UNAVAILABLE, &[]));
        assert!(is_retryable(
            StatusCode::FORBIDDEN,
            &["userRateLimitExceeded".into()]
        ));
        assert!(!is_retryable(
            StatusCode::FORBIDDEN,
            &["insufficientPermissions".into()]
        ));
        assert!(!is_retryable(StatusCode::NOT_FOUND, &[]));
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
