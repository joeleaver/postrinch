//! Reading a provider's answer: the body (up to a limit), `Retry-After`,
//! problem details and JSON.

use std::time::{Duration, SystemTime};

use bytes::{Bytes, BytesMut};
use ofscp_core::wire::ProblemDetails;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde::de::DeserializeOwned;

use crate::{Error, Problem, Result};

/// A provider's answer, read in full.
#[derive(Debug)]
pub(crate) struct Reply {
    pub authority: String,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Reply {
    /// The body as `T`.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|source| Error::Decode {
            authority: self.authority.clone(),
            expected: short_type_name::<T>(),
            source,
        })
    }

    /// Fail unless the status is `expected`.
    pub fn expect(self, expected: u16, spec: &'static str) -> Result<Self> {
        if self.status == expected {
            Ok(self)
        } else {
            Err(Error::UnexpectedStatus {
                authority: self.authority,
                status: self.status,
                expected: spec,
            })
        }
    }
}

/// The last path segment of a type's name, for error messages.
fn short_type_name<T>() -> &'static str {
    let full = std::any::type_name::<T>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base)
}

/// Read `response`'s body, refusing more than `limit` bytes.
pub(crate) async fn read_capped(
    authority: &str,
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Bytes> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(Error::TooLarge {
            authority: authority.to_owned(),
            limit,
        });
    }
    let mut body = BytesMut::new();
    while let Some(chunk) = response.chunk().await.map_err(|source| Error::Network {
        authority: authority.to_owned(),
        source,
    })? {
        if body.len() + chunk.len() > limit {
            return Err(Error::TooLarge {
                authority: authority.to_owned(),
                limit,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

/// The error a non-success answer means: its problem details when the body
/// is one, else just its status (§2.6: a proxy's answer may have no body).
pub(crate) fn problem(reply: &Reply) -> Error {
    let details = serde_json::from_slice::<ProblemDetails>(&reply.body).ok();
    let mut problem = Problem::new(&reply.authority, reply.status, details);
    // The status line is the truth; a body may claim another.
    problem.status = reply.status;
    problem.retry_after = retry_after(&reply.headers);
    Error::Problem(Box::new(problem))
}

/// `Retry-After` (RFC 9110 §10.2.3): seconds, or a date, measured against
/// the response's own `Date` so this machine's clock does not matter.
pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    let now = headers
        .get(reqwest::header::DATE)
        .and_then(|d| d.to_str().ok())
        .and_then(|d| httpdate::parse_http_date(d).ok())
        .unwrap_or_else(SystemTime::now);
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use reqwest::header::{DATE, HeaderValue};

    use super::*;

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            authority: "a.example".into(),
            status,
            headers: HeaderMap::new(),
            body: Bytes::from(body.to_owned()),
        }
    }

    #[test]
    fn retry_after_in_seconds() {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_static("120"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(120)));
    }

    #[test]
    fn retry_after_as_a_date_is_measured_from_the_response_date() {
        let mut h = HeaderMap::new();
        h.insert(
            DATE,
            HeaderValue::from_static("Wed, 01 Jan 2025 12:00:00 GMT"),
        );
        h.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Wed, 01 Jan 2025 12:05:00 GMT"),
        );
        assert_eq!(retry_after(&h), Some(Duration::from_secs(300)));
        h.insert(RETRY_AFTER, HeaderValue::from_static("soon"));
        assert_eq!(retry_after(&h), None);
    }

    #[test]
    fn a_problem_body_is_read() {
        let r = reply(
            403,
            r#"{"type":"https://a.example/problems/forbidden","status":403,"errorCode":"forbidden","detail":"no"}"#,
        );
        let Error::Problem(p) = problem(&r) else {
            panic!()
        };
        assert_eq!(p.error_code, Some(ofscp_core::wire::ErrorCode::Forbidden));
        assert_eq!(p.detail.as_deref(), Some("no"));
    }

    #[test]
    fn a_bare_status_is_a_problem_too() {
        let Error::Problem(p) = problem(&reply(502, "<html>Bad gateway</html>")) else {
            panic!()
        };
        assert_eq!((p.status, p.error_code.clone()), (502, None));
    }

    #[test]
    fn a_body_that_is_not_the_type_is_a_decode_error() {
        let err = reply(200, "{}")
            .json::<ofscp_core::wire::DeviceKeyCreated>()
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::Decode {
                    expected: "DeviceKeyCreated",
                    ..
                }
            ),
            "{err}"
        );
    }
}
