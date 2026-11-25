//! The loopback redirect: a one-shot HTTP listener on this machine that
//! receives the provider's redirect at the end of the browser sign-in.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use url::Url;

use super::first_line;
use crate::{SyncError, SyncResult};

/// The most of a request line and headers read before giving up on it.
const MAX_REQUEST: usize = 16 * 1024;
/// Ports tried for a `localhost` redirect before giving up on finding one
/// free on both 127.0.0.1 and ::1.
const PORT_ATTEMPTS: usize = 8;
/// How long a connection has to send its request line and headers. A
/// browser sends its redirect at once; one that opened a connection ahead
/// of time and sent nothing is dropped.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
/// The pause after a failed accept, so a failure that repeats (no file
/// descriptors left) does not spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Postrinch</title>\
    <body style=\"font-family:sans-serif;margin:3em\"><h1>Signed in</h1>\
    <p>You can close this tab and go back to Postrinch.</p>";
const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Postrinch</title>\
    <body style=\"font-family:sans-serif;margin:3em\"><h1>Sign-in did not finish</h1>\
    <p>Go back to Postrinch to try again.</p>";

/// How the redirect URI names this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectHost {
    /// `http://127.0.0.1:<port>/`, as Google asks of desktop clients.
    Ip,
    /// `http://localhost:<port>`, as Microsoft registers desktop clients
    /// (it accepts any port there). A browser may resolve `localhost` to
    /// ::1 first, so the listener is on both 127.0.0.1 and ::1.
    Localhost,
}

pub(crate) struct Loopback {
    v4: TcpListener,
    /// The same port on ::1, for a `localhost` redirect on a machine with
    /// IPv6.
    v6: Option<TcpListener>,
    redirect_uri: String,
}

impl Loopback {
    /// Listen on an ephemeral port.
    pub(crate) async fn bind(host: RedirectHost) -> SyncResult<Self> {
        if host == RedirectHost::Ip {
            let v4 = TcpListener::bind(("127.0.0.1", 0)).await?;
            let port = v4.local_addr()?.port();
            return Ok(Self {
                v4,
                v6: None,
                redirect_uri: format!("http://127.0.0.1:{port}/"),
            });
        }
        // The port must be ours on ::1 too: a browser that tries ::1 first
        // would otherwise hand the code to whatever holds it there.
        for _ in 0..PORT_ATTEMPTS {
            let v4 = TcpListener::bind(("127.0.0.1", 0)).await?;
            let port = v4.local_addr()?.port();
            let v6 = match TcpListener::bind(("::1", port)).await {
                Ok(v6) => Some(v6),
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
                // No IPv6 here, so no browser reaches ::1 either.
                Err(e) => {
                    tracing::debug!("no loopback listener on ::1: {e}");
                    None
                }
            };
            return Ok(Self {
                v4,
                v6,
                redirect_uri: format!("http://localhost:{port}"),
            });
        }
        Err(SyncError::SignIn(
            "no free port for the sign-in's redirect on this machine".into(),
        ))
    }

    pub(crate) fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Wait for the redirect carrying `expected_state` and return its
    /// authorization code. `provider` names the sign-in page in an error
    /// it redirects with.
    ///
    /// Connections are served side by side, so one that never sends (a
    /// browser's preconnect, the address of `localhost` it tried first)
    /// holds nothing up; each gets [`CONNECTION_TIMEOUT`] to send its
    /// request. A connection that fails, sends something that is not a
    /// request, or asks for something else (a browser's `/favicon.ico`, a
    /// stale tab's redirect) is not the sign-in's and is answered or
    /// dropped while listening goes on. The first redirect with the
    /// expected state ends the wait, and the listeners close.
    pub(crate) async fn wait_for_code(
        self,
        expected_state: &str,
        provider: &'static str,
        timeout: Duration,
    ) -> SyncResult<String> {
        let state: Arc<str> = Arc::from(expected_state);
        tokio::time::timeout(timeout, self.serve_until_redirect(state, provider))
            .await
            .map_err(|_| SyncError::SignIn("timed out waiting for the browser".into()))?
    }

    async fn accept(&self) -> std::io::Result<TcpStream> {
        let (stream, _) = match &self.v6 {
            None => self.v4.accept().await?,
            Some(v6) => tokio::select! {
                accepted = self.v4.accept() => accepted?,
                accepted = v6.accept() => accepted?,
            },
        };
        Ok(stream)
    }

    async fn serve_until_redirect(
        &self,
        state: Arc<str>,
        provider: &'static str,
    ) -> SyncResult<String> {
        // Dropped on return, and with it any connection still being served.
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted = self.accept() => match accepted {
                    Ok(stream) => {
                        connections.spawn(serve(stream, Arc::clone(&state), provider));
                    }
                    Err(e) => {
                        // A connection that went away before it was taken,
                        // or the process out of descriptors for a moment.
                        tracing::debug!("loopback accept failed: {e}");
                        tokio::time::sleep(ACCEPT_BACKOFF).await;
                    }
                },
                Some(served) = connections.join_next() => match served {
                    Ok(Some(Ours::Code(code))) => return Ok(code),
                    Ok(Some(Ours::Error(e))) => return Err(SyncError::SignIn(e)),
                    Ok(None) => {}
                    Err(e) => tracing::debug!("a loopback connection's task failed: {e}"),
                },
            }
        }
    }
}

/// The sign-in's own redirect.
enum Ours {
    Code(String),
    Error(String),
}

/// Serve one connection: the sign-in's redirect, answered and returned, or
/// `None` for anything else, which is answered (or, if it fails or sends
/// no request in time, dropped).
async fn serve(mut stream: TcpStream, state: Arc<str>, provider: &'static str) -> Option<Ours> {
    let read = tokio::time::timeout(CONNECTION_TIMEOUT, read_request_target(&mut stream)).await;
    let target = match read {
        Ok(Ok(target)) => target,
        Ok(Err(e)) => {
            tracing::debug!("a loopback connection failed: {e}");
            return None;
        }
        Err(_) => {
            tracing::debug!("a loopback connection sent no request in time");
            return None;
        }
    };
    let Some(target) = target else {
        respond(&mut stream, "400 Bad Request", FAILED_PAGE).await;
        return None;
    };
    match redirect_result(&target, &state, provider) {
        Redirect::NotOurs => {
            respond(&mut stream, "404 Not Found", "").await;
            None
        }
        Redirect::Code(code) => {
            respond(&mut stream, "200 OK", DONE_PAGE).await;
            Some(Ours::Code(code))
        }
        Redirect::Error(e) => {
            respond(&mut stream, "200 OK", FAILED_PAGE).await;
            Some(Ours::Error(e))
        }
    }
}

#[derive(Debug, PartialEq)]
enum Redirect {
    NotOurs,
    Code(String),
    Error(String),
}

/// Classify a request target (`/?code=…&state=…`). `provider` names the
/// sign-in page in an error that comes without a description.
fn redirect_result(target: &str, expected_state: &str, provider: &str) -> Redirect {
    let Ok(url) = Url::parse("http://127.0.0.1").and_then(|base| base.join(target)) else {
        return Redirect::NotOurs;
    };
    if url.path() != "/" {
        return Redirect::NotOurs;
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    if param("state").as_deref() != Some(expected_state) {
        return Redirect::NotOurs;
    }
    if let Some(error) = param("error") {
        // Microsoft explains itself (`AADSTS65004: User declined…`);
        // Google sends only the code.
        let description = param("error_description")
            .map(|d| first_line(&d))
            .filter(|d| !d.is_empty());
        return Redirect::Error(match (error.as_str(), description) {
            (_, Some(description)) => description,
            ("access_denied", None) => "access was not granted".into(),
            (other, None) => format!("{provider} answered {other}"),
        });
    }
    match param("code") {
        Some(code) if !code.is_empty() => Redirect::Code(code),
        _ => Redirect::Error("the redirect had no code".into()),
    }
}

/// The target of a `GET` request line, or `None` for anything else.
async fn read_request_target(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > MAX_REQUEST {
            return Ok(None);
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next().unwrap_or("");
    let mut parts = line.split(' ');
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) => Ok(Some(target.to_owned())),
        _ => Ok(None),
    }
}

/// Answer the browser. A failure to write is not an error for sign-in: the
/// code, if any, has already been read.
async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    if let Err(e) = stream.write_all(response.as_bytes()).await {
        tracing::debug!("loopback response not delivered: {e}");
    }
}

#[cfg(test)]
mod tests;
