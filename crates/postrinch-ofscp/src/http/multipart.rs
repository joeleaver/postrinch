//! A `multipart/form-data` body (RFC 7578) whose bytes are known before it
//! is sent. The signature's content digest covers the whole body (§5.8),
//! so the body is hashed first, and then streamed from the same sources,
//! as many times as a `503` asks (§4.5), without holding a file in memory.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use bytes::Bytes;
use futures_util::stream::{self, BoxStream, StreamExt, TryStreamExt};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::{Error, Result};

/// Where a part's bytes come from.
#[derive(Debug, Clone)]
pub enum Source {
    /// Bytes in memory, such as a pasted picture.
    Bytes(Bytes),
    /// A file, read as it is sent.
    File(PathBuf),
}

/// How far an upload has got: bytes handed to the connection so far, of
/// the whole body. A retry starts again from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes sent.
    pub sent: u64,
    /// The body's length.
    pub total: u64,
}

/// Told of an upload's progress as it goes.
pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

/// One part of the form.
pub(crate) enum Part {
    Text {
        name: &'static str,
        value: String,
    },
    File {
        name: &'static str,
        filename: String,
        mime: String,
        source: Source,
    },
}

#[derive(Clone)]
enum Segment {
    Bytes(Bytes),
    File { path: PathBuf, len: u64 },
}

/// A body ready to send: its segments, length and content digest.
pub(crate) struct Upload {
    boundary: String,
    segments: Arc<[Segment]>,
    len: u64,
    digest: String,
    progress: Option<ProgressFn>,
}

impl Upload {
    /// Lay out `parts`, read every file once to hash it, and remember each
    /// file's length.
    pub async fn prepare(parts: Vec<Part>, progress: Option<ProgressFn>) -> Result<Self> {
        let boundary = boundary();
        let mut segments = Vec::new();
        for part in parts {
            let mut head = format!("--{boundary}\r\n");
            match part {
                Part::Text { name, value } => {
                    head.push_str(&format!(
                        "Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"
                    ));
                    head.push_str(&value);
                    head.push_str("\r\n");
                    segments.push(Segment::Bytes(head.into()));
                }
                Part::File {
                    name,
                    filename,
                    mime,
                    source,
                } => {
                    head.push_str(&format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{}\"\r\n\
                         Content-Type: {}\r\n\r\n",
                        escape_quoted(&filename),
                        header_safe(&mime)?,
                    ));
                    segments.push(Segment::Bytes(head.into()));
                    segments.push(match source {
                        Source::Bytes(bytes) => Segment::Bytes(bytes),
                        Source::File(path) => {
                            let len = file_len(&path).await?;
                            Segment::File { path, len }
                        }
                    });
                    segments.push(Segment::Bytes(Bytes::from_static(b"\r\n")));
                }
            }
        }
        segments.push(Segment::Bytes(format!("--{boundary}--\r\n").into()));
        let (len, digest) = hash(&segments).await?;
        Ok(Self {
            boundary,
            segments: segments.into(),
            len,
            digest,
            progress,
        })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    /// A fresh stream of the body, for one attempt, and the count of its
    /// bytes handed to the connection so far.
    pub fn body(&self) -> (reqwest::Body, Arc<AtomicU64>) {
        let total = self.len;
        let sent = Arc::new(AtomicU64::new(0));
        let counter = sent.clone();
        let progress = self.progress.clone();
        let stream = stream::iter(self.segments.to_vec())
            .flat_map(segment_stream)
            .inspect_ok(move |chunk| {
                let now =
                    sent.fetch_add(chunk.len() as u64, Ordering::Relaxed) + chunk.len() as u64;
                if let Some(progress) = &progress {
                    progress(Progress { sent: now, total });
                }
            });
        (reqwest::Body::wrap_stream(stream), counter)
    }
}

/// A random boundary: 128 bits, so it never occurs in the content.
fn boundary() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("ofscp-{hex}")
}

/// A filename inside a quoted header parameter, escaped as HTML forms do.
fn escape_quoted(s: &str) -> String {
    s.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// A header value with no line breaks.
fn header_safe(s: &str) -> Result<&str> {
    if s.bytes().any(|b| b == b'\r' || b == b'\n') {
        return Err(Error::InvalidUpload("a MIME type with a line break"));
    }
    Ok(s)
}

async fn file_len(path: &PathBuf) -> Result<u64> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|source| read_error(path, source))?;
    Ok(meta.len())
}

fn read_error(path: &std::path::Path, source: io::Error) -> Error {
    Error::Read {
        path: path.to_owned(),
        source,
    }
}

/// The body's length and its `base64(SHA-256)` (§4.4.1).
async fn hash(segments: &[Segment]) -> Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut len = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    for segment in segments {
        match segment {
            Segment::Bytes(bytes) => {
                hasher.update(bytes);
                len += bytes.len() as u64;
            }
            Segment::File {
                path,
                len: expected,
            } => {
                let mut file = tokio::fs::File::open(path)
                    .await
                    .map_err(|e| read_error(path, e))?;
                let mut read = 0u64;
                loop {
                    let n = file.read(&mut buf).await.map_err(|e| read_error(path, e))?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                    read += n as u64;
                }
                if read != *expected {
                    return Err(read_error(path, changed()));
                }
                len += read;
            }
        }
    }
    Ok((
        len,
        base64::engine::general_purpose::STANDARD.encode(hasher.finalize()),
    ))
}

fn changed() -> io::Error {
    io::Error::other("the file changed while it was being uploaded")
}

/// One segment's bytes. A file must still be the length it was when it
/// was hashed, or the provider would refuse the digest; it fails here
/// first, naming the cause.
fn segment_stream(segment: Segment) -> BoxStream<'static, io::Result<Bytes>> {
    match segment {
        Segment::Bytes(bytes) => stream::once(async move { Ok(bytes) }).boxed(),
        Segment::File { path, len } => {
            let read = Arc::new(AtomicU64::new(0));
            let counted = read.clone();
            let file = stream::once(async move {
                let file = tokio::fs::File::open(&path).await?;
                // One byte more than expected shows a file that grew.
                Ok::<_, io::Error>(ReaderStream::new(file.take(len + 1)))
            })
            .try_flatten()
            .inspect_ok(move |chunk| {
                counted.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            });
            let check = stream::once(async move {
                if read.load(Ordering::Relaxed) == len {
                    Ok(None)
                } else {
                    Err(changed())
                }
            });
            file.map_ok(Some)
                .chain(check)
                .try_filter_map(|c| async move { Ok(c) })
                .boxed()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    async fn collect(upload: &Upload) -> io::Result<Vec<u8>> {
        let segments = upload.segments.to_vec();
        let chunks: Vec<Bytes> = stream::iter(segments)
            .flat_map(segment_stream)
            .try_collect()
            .await?;
        Ok(chunks.concat())
    }

    fn file_part(source: Source) -> Part {
        Part::File {
            name: "file",
            filename: "a \"b\".png".into(),
            mime: "image/png".into(),
            source,
        }
    }

    #[tokio::test]
    async fn the_body_is_what_was_hashed() {
        let dir = std::env::temp_dir().join(format!("postrinch-ofscp-mp-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("pic.png");
        tokio::fs::write(&path, vec![9u8; 200_000]).await.unwrap();
        let upload = Upload::prepare(
            vec![
                file_part(Source::File(path.clone())),
                Part::Text {
                    name: "target",
                    value: "b.example".into(),
                },
            ],
            None,
        )
        .await
        .unwrap();
        let body = collect(&upload).await.unwrap();
        assert_eq!(body.len() as u64, upload.len());
        assert_eq!(ofscp_core::signing::content_digest(&body), upload.digest());
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with(&format!("--{}\r\n", upload.boundary)));
        assert!(text.contains(
            "name=\"file\"; filename=\"a %22b%22.png\"\r\nContent-Type: image/png\r\n\r\n"
        ));
        assert!(text.contains("name=\"target\"\r\n\r\nb.example\r\n"));
        assert!(text.ends_with(&format!("--{}--\r\n", upload.boundary)));
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn a_file_that_changes_fails_the_upload() {
        let dir = std::env::temp_dir().join(format!("postrinch-ofscp-mp2-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("grows.bin");
        tokio::fs::write(&path, vec![1u8; 1000]).await.unwrap();
        let upload = Upload::prepare(vec![file_part(Source::File(path.clone()))], None)
            .await
            .unwrap();
        tokio::fs::write(&path, vec![1u8; 1001]).await.unwrap();
        assert!(collect(&upload).await.is_err());
        tokio::fs::write(&path, vec![1u8; 999]).await.unwrap();
        assert!(collect(&upload).await.is_err());
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn progress_counts_up_to_the_total() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let upload = Upload::prepare(
            vec![file_part(Source::Bytes(Bytes::from(vec![0u8; 5000])))],
            Some(Arc::new(move |p| log.lock().unwrap().push(p))),
        )
        .await
        .unwrap();
        let (body, sent) = upload.body();
        let bytes: Vec<Bytes> = http_body_util::BodyDataStream::new(body)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(bytes.concat().len() as u64, upload.len());
        assert_eq!(sent.load(Ordering::Relaxed), upload.len());
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.last(),
            Some(&Progress {
                sent: upload.len(),
                total: upload.len()
            })
        );
        assert!(seen.windows(2).all(|w| w[0].sent < w[1].sent));
    }

    #[test]
    fn a_mime_type_cannot_break_the_headers() {
        assert!(header_safe("image/png\r\nX-Evil: 1").is_err());
        assert_eq!(header_safe("image/png").unwrap(), "image/png");
    }
}
