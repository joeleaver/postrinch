//! Sending one request: signing it (§4.4), retrying a `503` with a fresh
//! signature (§4.5), correcting for the provider's clock, and reading the
//! answer.

use bytes::Bytes;
use ofscp_core::signing::{
    CanonicalRequest, EMPTY_BODY_DIGEST, SignedHeaders, content_digest, generate_nonce,
    signing_timestamp,
};
use rand_core::OsRng;
use reqwest::Method;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, DATE, LOCATION};
use url::Url;

use super::multipart::Upload;
use super::path::Query;
use super::response::{self, Reply};
use super::watch::{self, Watch};
use crate::{Error, Identity, Result};

/// How a request proves who sends it.
#[derive(Clone, Copy)]
pub(crate) enum Auth<'a> {
    /// Not at all: discovery, login, an anonymous read (§4.5).
    Anonymous,
    /// The bootstrap token, for registering a device key only (§4.2).
    Bearer(&'a str),
    /// The device key's signature (§4.4).
    Signed(&'a Identity),
}

/// What a request carries.
pub(crate) enum Body<'a> {
    Empty,
    Json(Bytes),
    Upload(&'a Upload),
}

impl Body<'_> {
    /// The content digest, computed once for every attempt (§4.4.1).
    fn digest(&self) -> String {
        match self {
            Self::Empty => EMPTY_BODY_DIGEST.to_owned(),
            Self::Json(bytes) => content_digest(bytes),
            Self::Upload(upload) => upload.digest().to_owned(),
        }
    }
}

/// One request, before it is signed.
pub(crate) struct Call<'a> {
    pub method: Method,
    pub path: String,
    pub query: Query,
    pub body: Body<'a>,
}

impl<'a> Call<'a> {
    pub fn new(method: Method, path: String) -> Self {
        Self {
            method,
            path,
            query: Query::new(),
            body: Body::Empty,
        }
    }

    pub fn query(mut self, query: Query) -> Self {
        self.query = query;
        self
    }

    pub fn json<B: serde::Serialize + ?Sized>(mut self, body: &B) -> Result<Self> {
        self.body = Body::Json(Bytes::from(
            serde_json::to_vec(body).map_err(Error::Encode)?,
        ));
        Ok(self)
    }

    pub fn upload(mut self, upload: &'a Upload) -> Self {
        self.body = Body::Upload(upload);
        self
    }
}

impl super::Provider {
    /// Send `call`, retrying as the spec allows, and read the answer. A
    /// non-success status is an [`Error::Problem`].
    pub(crate) async fn execute(&self, auth: Auth<'_>, call: Call<'_>) -> Result<Reply> {
        let url = self.url(&call)?;
        let authority = self.endpoint().authority().to_string();
        let digest = call.body.digest();
        let mut retry = 0;
        let mut resigned_for_clock = false;
        loop {
            let offset_at_signing = self.clock().offset_secs();
            let signed_at = self.clock().now();
            let request = self.build(auth, &call, &url, &digest, signed_at)?;
            let options = self.transport().options();
            let response = match &call.body {
                Body::Upload(upload) => {
                    let (body, sent) = upload.body();
                    let watch = Watch::new(
                        upload.len(),
                        options.read_timeout,
                        options.request_timeout,
                        tokio::time::Instant::now(),
                    );
                    watch::send_upload(request.body(body), sent, watch, &authority).await?
                }
                _ => request.send().await.map_err(|source| Error::Network {
                    authority: authority.clone(),
                    source,
                })?,
            };
            if let Some(date) = response.headers().get(DATE).and_then(|d| d.to_str().ok()) {
                self.clock().observe_date(date);
            }
            let status = response.status().as_u16();
            if response.status().is_redirection() {
                return Err(Error::Redirect {
                    authority,
                    status,
                    location: response
                        .headers()
                        .get(LOCATION)
                        .and_then(|l| l.to_str().ok())
                        .map(str::to_owned),
                });
            }
            let headers = response.headers().clone();
            let limit = options.max_response_bytes;
            let body = match &call.body {
                // The upload client has no read timeout; bound the answer.
                Body::Upload(_) => tokio::time::timeout(
                    options.request_timeout,
                    response::read_capped(&authority, response, limit),
                )
                .await
                .map_err(|_| Error::Stalled {
                    authority: authority.clone(),
                    waiting_for: "the answer to the upload",
                    after: options.request_timeout,
                })??,
                _ => response::read_capped(&authority, response, limit).await?,
            };
            let reply = Reply {
                authority: authority.clone(),
                status,
                headers,
                body,
            };
            if (200..300).contains(&status) {
                return Ok(reply);
            }
            // §4.5: a 503 is retried, signed again with a fresh timestamp
            // and nonce; resending the same signature can never succeed.
            if status == 503 {
                let after = response::retry_after(&reply.headers);
                if let Some(wait) = options.retry.wait(retry, after) {
                    tracing::debug!(%authority, ?wait, retry, "503, retrying");
                    tokio::time::sleep(wait).await;
                    retry += 1;
                    continue;
                }
            }
            // §4.5 step 3: a signature outside ±300 s is a 401. If this
            // answer's Date shows the clock was the cause, sign once more on
            // the corrected clock. Only a moved offset counts: a slow
            // request is no sign of skew.
            if status == 401
                && matches!(auth, Auth::Signed(_))
                && !resigned_for_clock
                && self.clock().explains_a_rejection(offset_at_signing)
            {
                tracing::info!(%authority, "401 with a skewed clock, signing again");
                resigned_for_clock = true;
                continue;
            }
            return Err(response::problem(&reply));
        }
    }

    fn url(&self, call: &Call<'_>) -> Result<Url> {
        let mut url = self.endpoint().https(&call.path)?;
        if !call.query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(call.query.pairs().iter().map(|(k, v)| (*k, v.as_str())));
        }
        Ok(url)
    }

    fn build(
        &self,
        auth: Auth<'_>,
        call: &Call<'_>,
        url: &Url,
        digest: &str,
        signed_at: jiff::Timestamp,
    ) -> Result<reqwest::RequestBuilder> {
        let client = match call.body {
            Body::Upload(_) => self.transport().upload_http(),
            _ => self.transport().http(),
        };
        let mut request = client.request(call.method.clone(), url.clone());
        match auth {
            Auth::Anonymous => {}
            Auth::Bearer(token) => request = request.bearer_auth(token),
            Auth::Signed(identity) => {
                let headers = sign(identity, url, call.method.as_str(), digest, signed_at)?;
                for (name, value) in headers.pairs() {
                    request = request.header(name, value);
                }
            }
        }
        Ok(match &call.body {
            Body::Empty => request.timeout(self.transport().options().request_timeout),
            Body::Json(bytes) => request
                .timeout(self.transport().options().request_timeout)
                .header(CONTENT_TYPE, "application/json")
                .body(bytes.clone()),
            // No overall timeout: a large file takes as long as it takes.
            // `execute` adds the body, under a watch for stalls.
            Body::Upload(upload) => request
                .header(CONTENT_TYPE, upload.content_type())
                .header(CONTENT_LENGTH, upload.len()),
        })
    }
}

/// The signature headers for a request to `url`, signed over the path and
/// query exactly as they will be sent (§4.4.2).
fn sign(
    identity: &Identity,
    url: &Url,
    method: &str,
    digest: &str,
    signed_at: jiff::Timestamp,
) -> Result<SignedHeaders> {
    let timestamp = signing_timestamp(signed_at);
    let nonce = generate_nonce(&mut OsRng);
    let authority = authority_of(url);
    let canonical = CanonicalRequest {
        authority: &authority,
        method,
        path: url.path(),
        query: url.query(),
        timestamp: &timestamp,
        nonce: &nonce,
        content_digest: digest,
    }
    .render()?;
    Ok(SignedHeaders {
        actor: identity.actor().to_string(),
        key_id: identity.key_id().to_owned(),
        signature: identity.key().sign(&canonical),
        timestamp,
        nonce,
        content_digest: digest.to_owned(),
    })
}

/// The canonical authority of an `https:` URL built from an [`Endpoint`]:
/// its host, and its port when not 443 (`Url` drops a default port).
///
/// [`Endpoint`]: crate::Endpoint
fn authority_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use ofscp_core::Actor;
    use ofscp_core::signing::{DeviceKey, verify};

    use super::*;

    #[test]
    fn the_signature_covers_what_is_sent() {
        let identity = Identity::new(
            Actor::canonicalize("alice@a.example:8443").unwrap(),
            "dk_1".into(),
            DeviceKey::from_seed(&[7; 32]),
        );
        let url =
            Url::parse("https://a.example:8443/api/users/bob%40b.example/profile?z=1&a=2").unwrap();
        let at = jiff::Timestamp::from_second(1_767_268_800).unwrap();
        let headers = sign(&identity, &url, "GET", EMPTY_BODY_DIGEST, at).unwrap();
        assert_eq!(headers.timestamp, "2026-01-01T12:00:00Z");
        assert_eq!(authority_of(&url), "a.example:8443");
        let canonical = CanonicalRequest {
            authority: "a.example:8443",
            method: "GET",
            path: "/api/users/bob%40b.example/profile",
            query: Some("z=1&a=2"),
            timestamp: &headers.timestamp,
            nonce: &headers.nonce,
            content_digest: EMPTY_BODY_DIGEST,
        }
        .render()
        .unwrap();
        assert!(verify(
            &identity.key().verifying_key(),
            &canonical,
            &headers.signature
        ));
    }

    #[test]
    fn a_default_port_is_left_out() {
        assert_eq!(
            authority_of(&Url::parse("https://a.example:443/api").unwrap()),
            "a.example"
        );
    }
}
