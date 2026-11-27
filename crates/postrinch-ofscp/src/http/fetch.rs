//! Fetching a picture, video or file by its URL (§5.8 *Serving
//! attachments*, §6.3 avatars): an unsigned `GET` of a capability URL,
//! from whatever host serves it (the provider, a CDN, an object store).
//!
//! Unlike the OFSCP endpoints, a media URL may redirect: a provider may
//! serve a stable avatar or attachment URL as a redirect to a freshly
//! signed object-store URL (§5.8), which browsers follow, so this follows
//! a few, to `https` only. No credential is ever sent: the URL is the
//! capability.

use bytes::Bytes;
use reqwest::header::CONTENT_TYPE;

use super::response::{self, Reply};
use crate::{Error, Result, Transport};

/// The redirects one fetch follows.
pub(crate) const MAX_REDIRECTS: usize = 5;

/// The most of an error answer's body read, for its problem details.
const PROBLEM_BYTES: usize = 64 * 1024;

/// A fetched file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedMedia {
    /// Its bytes, whole.
    pub bytes: Bytes,
    /// The `Content-Type` it was served with. A claim of the host, like the
    /// attachment's own `mime`: never a reason to open it as a document.
    pub mime: Option<String>,
}

impl Transport {
    /// Fetch the file at `url`, refusing one over `limit` bytes
    /// ([`Error::TooLarge`]). A non-success answer is a
    /// [`Error::Problem`] with its status: `403`, `404` or `410` from a
    /// short-lived URL means reading the message again for a fresh one
    /// (§5.8 *Revocation*). The connection may go quiet for at most the
    /// read timeout; the whole fetch has no limit, so a large video can
    /// come over a slow line.
    pub async fn fetch_media(&self, url: &str, limit: u64) -> Result<FetchedMedia> {
        let parsed = url::Url::parse(url).map_err(|_| Error::MediaUrl {
            url: url.to_owned(),
            reason: "it does not read as a URL",
        })?;
        if parsed.scheme() != "https" {
            return Err(Error::MediaUrl {
                url: url.to_owned(),
                reason: "media is fetched over https only",
            });
        }
        let host = parsed.host_str().unwrap_or_default().to_owned();
        let authority = match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        let response = self
            .media_http()
            .get(parsed)
            .send()
            .await
            .map_err(|source| Error::Network {
                authority: authority.clone(),
                source,
            })?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        if !(200..300).contains(&status) {
            // The status is the answer; a body too large to be problem
            // details is left unread.
            let body = response::read_capped(&authority, response, PROBLEM_BYTES)
                .await
                .unwrap_or_default();
            let reply = Reply {
                authority,
                status,
                headers,
                body,
            };
            return Err(response::problem(&reply));
        }
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let bytes = response::read_capped(&authority, response, limit).await?;
        let mime = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(FetchedMedia { bytes, mime })
    }
}
