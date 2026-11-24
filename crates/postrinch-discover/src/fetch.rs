//! Fetching autoconfig files: a trait, so tests can serve them, and the real
//! HTTPS-only client.

use std::time::Duration;

use thiserror::Error;

use crate::BoxFuture;

/// The most a fetched autoconfig file may be. Real ones are a few
/// kilobytes.
pub const MAX_BODY: usize = 1024 * 1024;

/// The most redirects followed for one fetch.
pub const MAX_REDIRECTS: usize = 3;

/// Why a fetch gave no file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FetchError {
    /// The server says there is no such file (404 or 410).
    #[error("not found")]
    NotFound,
    /// The URL, or a redirect, is not HTTPS.
    #[error("refused a URL that is not HTTPS: {0}")]
    NotHttps(String),
    /// The server answered with another error status.
    #[error("the server answered {0}")]
    Status(u16),
    /// The file is larger than [`MAX_BODY`].
    #[error("the answer is larger than {MAX_BODY} bytes")]
    TooLarge,
    /// The file is not UTF-8 text.
    #[error("the answer is not UTF-8 text")]
    NotText,
    /// The server could not be reached, or the connection failed.
    #[error("{0}")]
    Http(String),
}

/// Fetches a URL's body as text.
pub trait Fetch: Send + Sync {
    /// GETs `url` and returns its body.
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, FetchError>>;
}

/// The real fetcher: HTTPS only, the platform's certificate verifier, at
/// most [`MAX_REDIRECTS`] redirects (each HTTPS), bodies up to
/// [`MAX_BODY`].
#[derive(Debug, Clone)]
pub struct HttpsFetch {
    client: reqwest::Client,
}

impl HttpsFetch {
    /// A fetcher whose requests each give up after `timeout`.
    pub fn new(timeout: Duration) -> Result<Self, FetchError> {
        let redirects = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if attempt.url().scheme() != "https" {
                let url = attempt.url().to_string();
                attempt.error(FetchError::NotHttps(url))
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(redirects)
            .timeout(timeout)
            .connect_timeout(timeout)
            .user_agent(concat!("Postrinch/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| FetchError::Http(e.to_string()))?;
        Ok(HttpsFetch { client })
    }

    async fn fetch(&self, url: &str) -> Result<String, FetchError> {
        let parsed = url::Url::parse(url).map_err(|e| FetchError::Http(e.to_string()))?;
        if parsed.scheme() != "https" {
            return Err(FetchError::NotHttps(url.to_owned()));
        }
        let mut response = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(|e| FetchError::Http(error_chain(&e)))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return Err(FetchError::NotFound);
        }
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err(FetchError::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::Http(error_chain(&e)))?
        {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        String::from_utf8(body).map_err(|_| FetchError::NotText)
    }
}

impl Fetch for HttpsFetch {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, FetchError>> {
        Box::pin(self.fetch(url))
    }
}

/// An error with its causes, which is where reqwest says what went wrong.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
