//! A small signing OFSCP client for the fake's own tests.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use ofscp_core::signing::{DeviceKey, Request, Signer, generate_nonce, signing_timestamp};
use ofscp_core::ws::{Authenticate, Command, Envelope, Event};
use ofscp_core::{Actor, Authority};
use postrinch_ofscp_fake::{FakeProvider, root_certificate_der};
use rand_core::OsRng;
use reqwest::{Method, StatusCode};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

/// An HTTP client that trusts the fakes' certificate.
pub fn http() -> reqwest::Client {
    let cert = reqwest::Certificate::from_der(root_certificate_der()).unwrap();
    reqwest::Client::builder()
        .tls_certs_only([cert])
        .no_proxy()
        .build()
        .unwrap()
}

/// A user of some provider with a registered device key.
pub struct User {
    pub actor: Actor,
    pub key_id: String,
    pub key: DeviceKey,
    pub http: reqwest::Client,
}

pub fn user(p: &FakeProvider, handle: &str) -> User {
    let actor = p.add_user(handle, "secret");
    let key = DeviceKey::generate(&mut OsRng);
    let key_id = p.add_device_key(&actor, key.verifying_key(), "test device");
    User {
        actor,
        key_id,
        key,
        http: http(),
    }
}

/// One response: status, headers, and the body as JSON (`Null` if none).
pub struct Reply {
    pub status: StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub json: Value,
}

pub async fn reply(response: reqwest::Response) -> Reply {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.bytes().await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply {
        status,
        headers,
        json,
    }
}

/// What to sign a request with, apart from the user's own key.
#[derive(Default, Clone)]
pub struct Tweak {
    /// Sign with this timestamp instead of now.
    pub timestamp: Option<jiff::Timestamp>,
    /// Use this nonce instead of a fresh one.
    pub nonce: Option<String>,
    /// Send this body but sign `signed_body`.
    pub signed_body: Option<Vec<u8>>,
}

impl User {
    /// A signed request to `to` (which may be another provider).
    pub async fn send(
        &self,
        to: &FakeProvider,
        method: Method,
        target: &str,
        body: Option<&Value>,
    ) -> Reply {
        self.send_with(to, method, target, body, Tweak::default())
            .await
    }

    pub async fn send_with(
        &self,
        to: &FakeProvider,
        method: Method,
        target: &str,
        body: Option<&Value>,
        tweak: Tweak,
    ) -> Reply {
        let bytes = body
            .map(|b| serde_json::to_vec(b).unwrap())
            .unwrap_or_default();
        let content_type = body.is_some().then_some("application/json");
        self.send_raw(to, method, target, bytes, content_type, tweak)
            .await
    }

    pub fn sign(
        &self,
        to: &Authority,
        method: &str,
        target: &str,
        body: &[u8],
        tweak: &Tweak,
    ) -> Vec<(&'static str, String)> {
        let (path, query) = target
            .split_once('?')
            .map_or((target, None), |(p, q)| (p, Some(q)));
        let signer = Signer {
            actor: &self.actor,
            key_id: &self.key_id,
            key: &self.key,
        };
        let request = Request {
            authority: to,
            method,
            path,
            query,
            body,
        };
        let timestamp = signing_timestamp(tweak.timestamp.unwrap_or_else(jiff::Timestamp::now));
        let nonce = tweak
            .nonce
            .clone()
            .unwrap_or_else(|| generate_nonce(&mut OsRng));
        let signed = signer.sign_request(&request, &timestamp, &nonce).unwrap();
        signed
            .pairs()
            .iter()
            .map(|(k, v)| (*k, (*v).to_owned()))
            .collect()
    }

    pub async fn send_raw(
        &self,
        to: &FakeProvider,
        method: Method,
        target: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
        tweak: Tweak,
    ) -> Reply {
        self.try_send_raw(to, method, target, body, content_type, tweak)
            .await
            .expect("an answer")
    }

    /// A signed JSON request to `to` that may get no answer.
    pub async fn try_send(
        &self,
        to: &FakeProvider,
        method: Method,
        target: &str,
        body: &Value,
    ) -> reqwest::Result<Reply> {
        let bytes = serde_json::to_vec(body).unwrap();
        let json = Some("application/json");
        self.try_send_raw(to, method, target, bytes, json, Tweak::default())
            .await
    }

    async fn try_send_raw(
        &self,
        to: &FakeProvider,
        method: Method,
        target: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
        tweak: Tweak,
    ) -> reqwest::Result<Reply> {
        let signed_body = tweak.signed_body.clone().unwrap_or_else(|| body.clone());
        let headers = self.sign(
            &to.authority(),
            method.as_str(),
            target,
            &signed_body,
            &tweak,
        );
        let mut request = self
            .http
            .request(method, format!("{}{target}", to.base_url()))
            .body(body);
        if let Some(ct) = content_type {
            request = request.header("content-type", ct);
        }
        for (name, value) in headers {
            request = request.header(name, value);
        }
        Ok(reply(request.send().await?).await)
    }
}

/// An unsigned request.
pub async fn anonymous(
    to: &FakeProvider,
    method: Method,
    target: &str,
    body: Option<&Value>,
) -> Reply {
    let mut request = http().request(method, format!("{}{target}", to.base_url()));
    if let Some(b) = body {
        request = request.json(b);
    }
    reply(request.send().await.unwrap()).await
}

/// A WebSocket client.
pub struct Ws {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next: u64,
    pub challenge: String,
}

fn connector() -> Connector {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root_certificate_der().to_vec().into()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    Connector::Rustls(Arc::new(config))
}

/// How a socket ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// A close frame with this code.
    Closed(u16),
    /// The socket went with no close frame.
    Dropped,
}

const WAIT: Duration = Duration::from_secs(5);

impl Ws {
    /// Connect and read the challenge.
    pub async fn connect(p: &FakeProvider) -> Ws {
        let (stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            p.ws_url(),
            None,
            false,
            Some(connector()),
        )
        .await
        .unwrap();
        let mut ws = Ws {
            stream,
            next: 0,
            challenge: String::new(),
        };
        match ws.event().await.body {
            Event::AuthChallenge(c) => ws.challenge = c.nonce,
            other => panic!("expected auth.challenge, got {other:?}"),
        }
        ws
    }

    /// Connect and authenticate as `user`.
    pub async fn signed_in(p: &FakeProvider, user: &User) -> Ws {
        let mut ws = Self::connect(p).await;
        ws.authenticate(p, user).await;
        match ws.event().await.body {
            Event::Authenticated(a) => assert_eq!(a.actor, user.actor),
            other => panic!("expected authenticated, got {other:?}"),
        }
        ws
    }

    pub async fn authenticate(&mut self, p: &FakeProvider, user: &User) -> String {
        let timestamp = signing_timestamp(jiff::Timestamp::now());
        let signer = Signer {
            actor: &user.actor,
            key_id: &user.key_id,
            key: &user.key,
        };
        let signature = signer
            .sign_ws_authenticate(&p.authority(), &self.challenge, &timestamp)
            .unwrap();
        self.send(Command::Authenticate(Authenticate {
            actor: user.actor.clone(),
            key_id: user.key_id.clone(),
            timestamp,
            signature,
        }))
        .await
    }

    /// Send a command; returns its id.
    pub async fn send(&mut self, body: Command) -> String {
        self.next += 1;
        let id = format!("cmd_{}", self.next);
        let envelope = Envelope {
            id: id.clone(),
            ts: Some(jiff::Timestamp::now()),
            correlation_id: None,
            body,
        };
        self.send_text(envelope.encode().unwrap()).await;
        id
    }

    pub async fn send_text(&mut self, text: String) {
        self.stream.send(Frame::text(text)).await.unwrap();
    }

    /// The next event, skipping pings; panics on timeout or close.
    pub async fn event(&mut self) -> Envelope<Event> {
        loop {
            let frame = tokio::time::timeout(WAIT, self.stream.next())
                .await
                .expect("an event within the wait")
                .expect("the socket is open")
                .expect("a frame");
            if let Frame::Text(text) = frame {
                let envelope = Envelope::<Event>::decode(&text).unwrap();
                if envelope.body.kind() != "ping" {
                    return envelope;
                }
            }
        }
    }

    /// Is no event waiting within `wait`?
    pub async fn quiet(&mut self, wait: Duration) -> bool {
        loop {
            match tokio::time::timeout(wait, self.stream.next()).await {
                Err(_) => return true,
                Ok(Some(Ok(Frame::Text(text)))) => {
                    let envelope = Envelope::<Event>::decode(&text).unwrap();
                    if envelope.body.kind() != "ping" {
                        return false;
                    }
                }
                Ok(_) => return false,
            }
        }
    }

    /// Read until the socket ends; events before the end are skipped.
    pub async fn ended(&mut self) -> Ended {
        loop {
            let next = tokio::time::timeout(WAIT, self.stream.next())
                .await
                .expect("the socket ends within the wait");
            match next {
                Some(Ok(Frame::Close(Some(frame)))) => return Ended::Closed(u16::from(frame.code)),
                Some(Ok(Frame::Close(None))) => return Ended::Closed(u16::from(CloseCode::Status)),
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return Ended::Dropped,
            }
        }
    }
}

/// One part of a form: `(name, filename, content type, bytes)`.
pub type Part<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a [u8]);

/// A `multipart/form-data` body. Returns the body and its content type.
pub fn multipart(parts: &[Part<'_>]) -> (Vec<u8>, String) {
    let boundary = "fake-boundary-7d1f";
    let mut body = Vec::new();
    for (name, filename, mime, bytes) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let mut disposition = format!("Content-Disposition: form-data; name=\"{name}\"");
        if let Some(f) = filename {
            disposition.push_str(&format!("; filename=\"{f}\""));
        }
        body.extend_from_slice(format!("{disposition}\r\n").as_bytes());
        if let Some(m) = mime {
            body.extend_from_slice(format!("Content-Type: {m}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/form-data; boundary={boundary}"))
}
