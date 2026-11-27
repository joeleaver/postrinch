//! What every route shares: who is calling, problem+json answers, JSON
//! bodies and the §7.2 paging parameters.

use std::collections::HashMap;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use ofscp_core::Actor;
use ofscp_core::wire::ABOUT_BLANK;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::inner::Inner;
use crate::problem::{Outcome, Problem};
use crate::state::timeline::PageQuery;
use crate::verify::Verified;

/// Who a request is from, as the gate established it (§4.5).
#[derive(Debug, Clone)]
pub(crate) enum Caller {
    /// No `X-OFSCP-*` header and no bearer token.
    Anonymous,
    /// `Authorization: Bearer <token>` (§4.2), not yet checked.
    Bearer(String),
    /// A signature that passed every check.
    Signed(Verified),
}

impl Caller {
    /// The viewer for read-access rules: `None` is anonymous.
    pub(crate) fn viewer(&self) -> Option<&Actor> {
        match self {
            Self::Signed(v) => Some(&v.actor),
            _ => None,
        }
    }

    /// The signer, for a route that needs a signature (`401` otherwise).
    pub(crate) fn signed(&self) -> Outcome<&Actor> {
        self.viewer()
            .ok_or_else(|| Problem::unauthorized("this route needs a signed request"))
    }

    /// The signer, for a signed local-account endpoint: a remote actor is
    /// answered `403`.
    pub(crate) fn local(&self, inner: &Inner) -> Outcome<Actor> {
        let actor = self.signed()?;
        if !actor.is_local_to(&inner.authority) {
            return Err(Problem::forbidden("a local-account endpoint"));
        }
        Ok(actor.clone())
    }
}

/// A route's answer.
pub(crate) type Reply = Outcome<Response>;

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let p = self.into_fields();
        let mut body = serde_json::json!({
            "type": ABOUT_BLANK,
            "title": status.canonical_reason().unwrap_or("Error"),
            "status": status.as_u16(),
            "errorCode": p.code,
            "detail": p.detail,
        });
        if let Some(details) = p.details {
            body["details"] = details;
        }
        let mut response = (status, body.to_string()).into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(secs) = p.retry_after {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        response
    }
}

/// `status` with `value` as JSON.
pub(crate) fn json<T: Serialize>(status: StatusCode, value: &T) -> Response {
    let text = serde_json::to_string(value).expect("wire types serialize");
    let mut response = (status, text).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

pub(crate) fn ok<T: Serialize>(value: &T) -> Reply {
    Ok(json(StatusCode::OK, value))
}

pub(crate) fn created<T: Serialize>(value: &T) -> Reply {
    Ok(json(StatusCode::CREATED, value))
}

pub(crate) fn no_content() -> Reply {
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// A JSON request body, `400` when it does not match.
pub(crate) fn body<T: DeserializeOwned>(bytes: &Bytes) -> Outcome<T> {
    serde_json::from_slice(bytes)
        .map_err(|e| Problem::bad_request(format!("the body does not match: {e}")))
}

/// An optional JSON body: empty means `T::default()`.
pub(crate) fn optional_body<T: DeserializeOwned + Default>(bytes: &Bytes) -> Outcome<T> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    body(bytes)
}

/// The paging parameters of §7.2, read as leniently as it asks: a bad
/// `limit` is the default, an unknown `direction` is `backward`.
pub(crate) fn page_query(params: &HashMap<String, String>, default_limit: usize) -> PageQuery {
    let limit = params
        .get("limit")
        .and_then(|l| l.trim().parse::<f64>().ok())
        .filter(|l| l.is_finite() && *l >= 1.0)
        .map_or(default_limit, |l| l.floor().min(100.0) as usize);
    PageQuery {
        cursor: params.get("cursor").filter(|c| !c.is_empty()).cloned(),
        forward: params.get("direction").is_some_and(|d| d == "forward"),
        limit,
    }
}
