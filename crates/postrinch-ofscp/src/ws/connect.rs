//! Opening a connection: TCP, TLS, the upgrade, and the challenge sign-in
//! (§7.1 "Authentication").

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use ofscp_core::signing::signing_timestamp;
use ofscp_core::ws::{Authenticate, Command, Envelope, Event, WsError};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::connection::{Connection, Events};
use super::driver::{self, Ids, Parts};
use super::{CloseReason, ConnectError, WsOptions};
use crate::http::Client;

/// The socket the driver owns.
pub(super) type Socket = WebSocketStream<TlsStream<TcpStream>>;

/// Open `client`'s provider's socket and sign in as `client`'s identity.
/// On success the connection is authenticated, and its task runs until
/// the connection ends or every [`Connection`] is dropped.
pub async fn connect(
    client: &Client,
    options: WsOptions,
) -> Result<(Connection, Events), ConnectError> {
    let mut socket = open(client, &options).await?;
    let mut ids = Ids::default();
    sign_in(&mut socket, client, &options, &mut ids).await?;
    let parts = Parts {
        authority: client.provider().endpoint().authority().clone(),
        actor: client.identity().actor().clone(),
        clock: client.provider().clock().clone(),
    };
    Ok(driver::spawn(socket, parts, options, ids))
}

async fn open(client: &Client, options: &WsOptions) -> Result<Socket, ConnectError> {
    let endpoint = client.provider().endpoint();
    let authority = endpoint.authority().to_string();
    let url = endpoint.ws()?;
    let transport = client.provider().transport();
    let connect_timeout = transport.options().connect_timeout;
    let io_error = |source| ConnectError::Connect {
        authority: authority.clone(),
        source,
    };
    let tcp = within(
        connect_timeout,
        &authority,
        "connect",
        TcpStream::connect((endpoint.host(), endpoint.port())),
    )
    .await?
    .map_err(io_error)?;
    let name = ServerName::try_from(endpoint.host().to_owned())
        .map_err(|e| io_error(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))?;
    let tls = TlsConnector::from(transport.tls().ws_config());
    let stream = within(
        connect_timeout,
        &authority,
        "finish TLS",
        tls.connect(name, tcp),
    )
    .await?
    .map_err(io_error)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(options.max_frame_bytes))
        .max_frame_size(Some(options.max_frame_bytes));
    let upgrade = tokio_tungstenite::client_async_with_config(url.as_str(), stream, Some(config));
    let (socket, _response) = within(connect_timeout, &authority, "upgrade", upgrade)
        .await?
        .map_err(|source| ConnectError::Upgrade {
            authority: authority.clone(),
            source: Box::new(source),
        })?;
    Ok(socket)
}

async fn within<F: Future>(
    limit: Duration,
    authority: &str,
    step: &'static str,
    f: F,
) -> Result<F::Output, ConnectError> {
    tokio::time::timeout(limit, f)
        .await
        .map_err(|_| ConnectError::TimedOut {
            authority: authority.to_owned(),
            step,
        })
}

/// The next event, skipping control frames. `Err` is how the socket ended.
async fn next_event(
    socket: &mut Socket,
    authority: &str,
) -> Result<Envelope<Event>, Result<CloseReason, ConnectError>> {
    loop {
        let message = match socket.next().await {
            None => return Err(Ok(CloseReason::Dropped("closed before signing in".into()))),
            Some(Err(e)) => return Err(Ok(CloseReason::Dropped(e.to_string()))),
            Some(Ok(message)) => message,
        };
        match message {
            Message::Text(text) => {
                return Envelope::<Event>::decode(text.as_str()).map_err(|e| {
                    Err(ConnectError::Protocol {
                        authority: authority.to_owned(),
                        detail: e.to_string(),
                    })
                });
            }
            Message::Close(frame) => return Err(Ok(CloseReason::from_frame(frame.as_ref()))),
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
}

async fn sign_in(
    socket: &mut Socket,
    client: &Client,
    options: &WsOptions,
    ids: &mut Ids,
) -> Result<(), ConnectError> {
    let authority = client.provider().endpoint().authority().to_string();
    let deadline = tokio::time::Instant::now() + options.auth_timeout;
    let refused = |error: Option<Box<WsError>>, close| ConnectError::Refused {
        authority: authority.clone(),
        error,
        close,
    };
    let ended = |end: Result<CloseReason, ConnectError>| match end {
        Ok(close) => refused(None, close),
        Err(e) => e,
    };

    // 1. The provider speaks first: `auth.challenge`.
    let first = within_until(
        deadline,
        &authority,
        "send its challenge",
        next_event(socket, &authority),
    )
    .await?
    .map_err(ended)?;
    let Event::AuthChallenge(challenge) = first.body else {
        return Err(ConnectError::Protocol {
            authority,
            detail: format!(
                "the first frame was {}, not auth.challenge",
                first.body.kind()
            ),
        });
    };
    // The challenge carries the provider's time; sign with it.
    let clock = client.provider().clock();
    if let Some(ts) = first.ts {
        clock.observe_time(ts);
    }

    // 2. `authenticate`, signed over the authority, the nonce and the time.
    let timestamp = signing_timestamp(clock.now());
    let identity = client.identity();
    let signature = identity.signer().sign_ws_authenticate(
        client.provider().endpoint().authority(),
        &challenge.nonce,
        &timestamp,
    )?;
    let id = ids.next();
    let frame = Envelope {
        id: id.clone(),
        ts: Some(clock.now()),
        correlation_id: None,
        body: Command::Authenticate(Authenticate {
            actor: identity.actor().clone(),
            key_id: identity.key_id().to_owned(),
            timestamp,
            signature,
        }),
    };
    socket
        .send(Message::Text(frame.encode()?.into()))
        .await
        .map_err(|e| refused(None, CloseReason::Dropped(e.to_string())))?;

    // 3. `authenticated`, or an `error` and a close.
    let mut error = None;
    loop {
        let event = match within_until(
            deadline,
            &authority,
            "answer the sign-in",
            next_event(socket, &authority),
        )
        .await
        {
            Ok(Ok(event)) => event,
            Ok(Err(end)) => return Err(ended(end).with_error(error)),
            Err(timeout) if error.is_some() => {
                return Err(refused(error, CloseReason::Dropped(timeout.to_string())));
            }
            Err(timeout) => return Err(timeout),
        };
        if event.correlation_id.as_deref() != Some(id.as_str()) {
            tracing::debug!(%authority, kind = event.body.kind(), "ignoring a frame before sign-in");
            continue;
        }
        match event.body {
            Event::Authenticated(ok) if &ok.actor == identity.actor() => return Ok(()),
            Event::Authenticated(ok) => {
                return Err(ConnectError::Protocol {
                    authority,
                    detail: format!("signed in as {}, not {}", ok.actor, identity.actor()),
                });
            }
            // The provider closes with 4001 next; wait for it.
            Event::Error(e) => error = Some(Box::new(e)),
            other => {
                return Err(ConnectError::Protocol {
                    authority,
                    detail: format!("answered authenticate with {}", other.kind()),
                });
            }
        }
    }
}

async fn within_until<F: Future>(
    deadline: tokio::time::Instant,
    authority: &str,
    step: &'static str,
    f: F,
) -> Result<F::Output, ConnectError> {
    tokio::time::timeout_at(deadline, f)
        .await
        .map_err(|_| ConnectError::TimedOut {
            authority: authority.to_owned(),
            step,
        })
}

impl ConnectError {
    /// Attach the `error` event seen before the close.
    fn with_error(self, seen: Option<Box<WsError>>) -> Self {
        match self {
            Self::Refused {
                authority,
                error,
                close,
            } => Self::Refused {
                authority,
                error: error.or(seen),
                close,
            },
            other => other,
        }
    }
}
