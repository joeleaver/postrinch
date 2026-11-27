//! The REST surface: the gate every request passes, then the routes.

mod auth;
mod dms;
mod feeds;
mod groups;
mod me;
mod media;
mod messages;
pub(crate) mod respond;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt as _;
use ofscp_core::signing::headers as signing_headers;
use ofscp_core::wire::ErrorCode;

use crate::inner::Inner;
use crate::problem::Problem;
use crate::record::{Authentication, RecordedRequest};
use crate::server::ConnKill;
use crate::verify::{SignedRequest, is_signed, verify_request};
use respond::Caller;

/// Bodies other than uploads are small JSON objects.
const MAX_JSON_BODY: usize = 1024 * 1024;
/// Room for the multipart framing and the small form parts beside `file`
/// and `thumbnail`.
const MULTIPART_SLACK: u64 = 64 * 1024;

pub(crate) fn router(inner: Arc<Inner>) -> Router {
    Router::new()
        .route("/.well-known/ofscp-provider", get(auth::discovery))
        .route(
            "/.well-known/ofscp/users/{handle}/keys",
            get(auth::user_keys),
        )
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login))
        .route(
            "/api/auth/device-keys",
            post(auth::register_key).get(auth::list_keys),
        )
        .route(
            "/api/auth/device-keys/{key_id}",
            axum::routing::delete(auth::revoke_key),
        )
        .route("/api/ws", get(crate::ws::upgrade))
        .merge(me::routes())
        .merge(feeds::routes())
        .merge(groups::routes())
        .merge(messages::routes())
        .merge(media::routes())
        .merge(dms::routes())
        .layer(middleware::from_fn_with_state(inner.clone(), gate))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(inner)
}

/// Every request: read the body (under the upload faults), answer the
/// signed-request faults, check a signature, then route; record the outcome
/// and stamp `Date` from the provider's clock.
async fn gate(State(inner): State<Arc<Inner>>, request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    let method = parts.method.as_str().to_owned();
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().map(str::to_owned);
    let upload = parts.method == axum::http::Method::POST && path == "/api/media";
    let kill = parts.extensions.get::<ConnKill>().cloned();
    let nonce = header_str(&parts.headers, signing_headers::NONCE).map(str::to_owned);

    let (authentication, response) = match read_body(&inner, body, upload, kill).await {
        Err(problem) => (Authentication::Unsigned, problem.into_response()),
        Ok(bytes) => {
            let (authentication, caller) = authenticate(&inner, &parts, &bytes).await;
            match caller {
                Err(response) => (authentication, response),
                Ok(caller) => {
                    parts.extensions.insert(caller);
                    parts.extensions.insert(RequestBody(bytes.clone()));
                    let request = Request::from_parts(parts.clone(), Body::from(bytes));
                    (authentication, next.run(request).await)
                }
            }
        }
    };
    let mut response = response;
    let date = httpdate::fmt_http_date(inner.clock.system_now());
    if let Ok(date) = HeaderValue::from_str(&date) {
        response.headers_mut().insert(header::DATE, date);
    }
    inner.log.lock().unwrap().push(RecordedRequest {
        method,
        path,
        query,
        headers: parts
            .headers
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_owned(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect(),
        nonce,
        authentication,
        status: response.status().as_u16(),
    });
    response
}

/// The request's body, as the gate read it (handlers take it from here so
/// the signed bytes and the parsed bytes are the same).
#[derive(Clone)]
pub(crate) struct RequestBody(pub(crate) Bytes);

fn header_str<'a>(map: &'a HeaderMap, name: &str) -> Option<&'a str> {
    map.get(name).and_then(|v| v.to_str().ok())
}

/// Decide who the caller is, or answer for the gate.
async fn authenticate(
    inner: &Inner,
    parts: &axum::http::request::Parts,
    body: &Bytes,
) -> (Authentication, Result<Caller, Response>) {
    if !is_signed(&parts.headers) {
        let bearer = header_str(&parts.headers, "authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t.trim().to_owned());
        return match bearer {
            Some(token) => (Authentication::Bearer, Ok(Caller::Bearer(token))),
            None => (Authentication::Unsigned, Ok(Caller::Anonymous)),
        };
    }
    if let Some(problem) = take_fault(inner, &parts.method, parts.uri.path()) {
        let status = problem.status();
        return (
            Authentication::Faulted { status },
            Err(problem.into_response()),
        );
    }
    let signed = SignedRequest {
        method: parts.method.as_str(),
        path: parts.uri.path(),
        query: parts.uri.query(),
        headers: &parts.headers,
        body,
    };
    match verify_request(inner, signed).await {
        Ok(verified) => (
            Authentication::Verified {
                actor: verified.actor.clone(),
                key_id: verified.key_id.clone(),
            },
            Ok(Caller::Signed(verified)),
        ),
        Err(problem) => (
            Authentication::Rejected {
                status: problem.status(),
                error_code: problem.code.as_str().to_owned(),
            },
            Err(problem.into_response()),
        ),
    }
}

/// The faults that answer a signed request before it is checked:
/// `refuse_signatures` (a 401 while on, for the paths it names),
/// `fail_reads` (a 500 for the reads it names), then `fail_next` (one 503
/// per signed request while it lasts).
fn take_fault(inner: &Inner, method: &axum::http::Method, path: &str) -> Option<Problem> {
    let mut faults = inner.faults.lock().unwrap();
    if faults
        .refuse_signatures
        .as_deref()
        .is_some_and(|suffix| path.ends_with(suffix))
    {
        return Some(Problem::unauthorized(
            "fault: signatures are refused (refuse_signatures)",
        ));
    }
    if method == axum::http::Method::GET
        && faults
            .fail_reads
            .as_deref()
            .is_some_and(|suffix| path.ends_with(suffix))
    {
        return Some(Problem::internal("fault: reading this fails (fail_reads)"));
    }
    if faults.fail_next == 0 {
        return None;
    }
    faults.fail_next -= 1;
    Some(Problem::unavailable(
        "fault: the provider is not ready (fail_next)",
        faults.fail_retry_after,
    ))
}

/// Read the whole body. An upload is read under the slow and cut faults,
/// and against the size limit (`413`).
async fn read_body(
    inner: &Inner,
    body: Body,
    upload: bool,
    kill: Option<ConnKill>,
) -> Result<Bytes, Problem> {
    let (limit, cut, rate) = if upload {
        let max = inner.settings().max_upload_bytes;
        let faults = inner.faults.lock().unwrap();
        let limit = max.saturating_mul(2).saturating_add(MULTIPART_SLACK);
        (
            usize::try_from(limit).unwrap_or(usize::MAX),
            faults.cut_uploads_after,
            faults.upload_bytes_per_sec,
        )
    } else {
        (MAX_JSON_BODY, None, None)
    };
    let mut stream = body.into_data_stream();
    let mut buf = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Problem::bad_request(format!("reading the body: {e}")))?;
        let received = buf.len() + chunk.len();
        if let Some(cut) = cut
            && received >= cut
        {
            // The connection goes, and this request with it.
            if let Some(kill) = &kill {
                kill.kill();
            }
            std::future::pending::<()>().await;
        }
        if received > limit {
            return Err(Problem::new(
                ErrorCode::PayloadTooLarge,
                "the body exceeds the provider's size limit",
            ));
        }
        buf.extend_from_slice(&chunk);
        if let Some(rate) = rate.filter(|r| *r > 0) {
            tokio::time::sleep(Duration::from_secs_f64(chunk.len() as f64 / rate as f64)).await;
        }
    }
    Ok(buf.freeze())
}
