//! Uploads and serving them (§5.8). An upload is stored whole, in memory;
//! its URL is a capability URL on this provider, so anyone holding it may
//! fetch the file, signed or not.

use std::sync::Arc;

use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use ofscp_core::wire::{Attachment, ErrorCode, Thumbnail};
use ofscp_core::{Actor, Authority};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

use super::respond::{Caller, Reply, json};
use crate::inner::Inner;
use crate::problem::{Outcome, Problem};
use crate::random::token;
use crate::state::Upload;

pub(super) fn routes() -> Router<Arc<Inner>> {
    Router::new()
        .route("/api/media", post(upload))
        .route("/media/{token}/{filename}", get(serve))
}

/// One file part of the form.
struct FilePart {
    filename: Option<String>,
    mime: String,
    bytes: Bytes,
}

/// What the form carried (§5.8 `POST /api/media`).
#[derive(Default)]
struct Form {
    file: Option<FilePart>,
    thumbnail: Option<FilePart>,
    target: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration_ms: Option<u64>,
}

fn bad_form(e: impl std::fmt::Display) -> Problem {
    Problem::bad_request(format!("not a multipart form: {e}"))
}

fn number<T: std::str::FromStr>(name: &str, text: &str) -> Outcome<T> {
    text.trim()
        .parse()
        .map_err(|_| Problem::bad_request(format!("{name} is not a number")))
}

async fn read_form(mut multipart: Multipart) -> Outcome<Form> {
    let mut form = Form::default();
    while let Some(field) = multipart.next_field().await.map_err(bad_form)? {
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "file" | "thumbnail" => {
                let part = FilePart {
                    filename: field.file_name().map(str::to_owned),
                    mime: field
                        .content_type()
                        .unwrap_or("application/octet-stream")
                        .to_owned(),
                    bytes: field.bytes().await.map_err(bad_form)?,
                };
                if name == "file" {
                    form.file = Some(part);
                } else {
                    form.thumbnail = Some(part);
                }
            }
            "target" => form.target = Some(field.text().await.map_err(bad_form)?),
            "width" => form.width = Some(number(&name, &field.text().await.map_err(bad_form)?)?),
            "height" => form.height = Some(number(&name, &field.text().await.map_err(bad_form)?)?),
            "durationMs" => {
                form.duration_ms = Some(number(&name, &field.text().await.map_err(bad_form)?)?)
            }
            // Unknown parts are ignored, as unknown fields are (§2.4).
            _ => {}
        }
    }
    Ok(form)
}

fn too_large(max: u64) -> Problem {
    Problem::new(
        ErrorCode::PayloadTooLarge,
        format!("files are limited to {max} bytes"),
    )
}

/// `sha-256:<base64>` (§5.8 `hash`).
fn hash(bytes: &[u8]) -> String {
    format!("sha-256:{}", STANDARD.encode(Sha256::digest(bytes)))
}

/// Store one file and return the attachment fields that describe it.
fn store(inner: &Inner, uploader: &Actor, target: &Authority, part: FilePart) -> Attachment {
    let mut state = inner.state.lock().unwrap();
    let id = state.ids.next("att_");
    let capability = token(16);
    let filename = part
        .filename
        .clone()
        .unwrap_or_else(|| id.as_str().to_owned());
    let attachment = Attachment {
        id: id.clone(),
        mime: part.mime,
        url: format!(
            "{}/media/{capability}/{}",
            inner.base_url(),
            utf8_percent_encode(&filename, NON_ALPHANUMERIC)
        ),
        size: part.bytes.len() as u64,
        filename: part.filename,
        hash: Some(hash(&part.bytes)),
        width: None,
        height: None,
        duration_ms: None,
        thumbnail: None,
        expires_at: None,
    };
    state.served.insert(capability, id.clone());
    state.uploads.insert(
        id,
        Upload {
            attachment: attachment.clone(),
            uploader: uploader.clone(),
            target: target.clone(),
            bytes: part.bytes,
        },
    );
    attachment
}

async fn upload(
    State(inner): State<Arc<Inner>>,
    Extension(caller): Extension<Caller>,
    multipart: Multipart,
) -> Reply {
    let uploader = caller.signed()?.clone();
    let form = read_form(multipart).await?;
    let max = inner.settings().max_upload_bytes;
    let file = form
        .file
        .ok_or_else(|| Problem::bad_request("the form has no file part"))?;
    if file.bytes.len() as u64 > max {
        return Err(too_large(max));
    }
    let target = match &form.target {
        Some(t) => Authority::canonicalize(t)
            .map_err(|_| Problem::bad_request("target is not a provider authority"))?,
        None => inner.authority.clone(),
    };
    let thumbnail = match form.thumbnail {
        Some(t) if !t.mime.starts_with("image/") => {
            return Err(Problem::bad_request("a thumbnail must be image/*"));
        }
        Some(t) if t.bytes.len() as u64 > max => return Err(too_large(max)),
        Some(t) => Some(store(&inner, &uploader, &target, t)),
        None => None,
    };
    let mut attachment = store(&inner, &uploader, &target, file);
    attachment.width = form.width;
    attachment.height = form.height;
    attachment.duration_ms = form.duration_ms;
    attachment.thumbnail = thumbnail.map(|t| Thumbnail {
        id: t.id,
        mime: t.mime,
        url: t.url,
        size: t.size,
        filename: t.filename,
        hash: t.hash,
        width: None,
        height: None,
        duration_ms: None,
        expires_at: None,
    });
    let mut state = inner.state.lock().unwrap();
    if let Some(u) = state.uploads.get_mut(&attachment.id) {
        u.attachment = attachment.clone();
    }
    Ok(json(StatusCode::CREATED, &attachment))
}

/// May the file be shown inline? Pictures, audio and video, except SVG,
/// which can carry script (§5.8 *Serving attachments*).
fn inline(mime: &str) -> bool {
    let media = ["image/", "audio/", "video/"]
        .iter()
        .any(|p| mime.starts_with(p));
    media && !mime.starts_with("image/svg")
}

async fn serve(
    State(inner): State<Arc<Inner>>,
    Path((capability, _)): Path<(String, String)>,
) -> Response {
    let state = inner.state.lock().unwrap();
    let Some(upload) = state
        .served
        .get(&capability)
        .and_then(|id| state.uploads.get(id))
    else {
        return Problem::not_found("no such file").into_response();
    };
    let mime = upload.attachment.mime.clone();
    let mut response = upload.bytes.clone().into_response();
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if !inline(&mime) {
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
    response
}
