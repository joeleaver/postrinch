//! How a connection ends: requests answered at once, the socket let go,
//! and the provider told why.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use tokio::io::ReadBuf;

use super::*;

#[tokio::test]
async fn a_request_after_the_end_fails_at_once_though_the_reader_is_behind() {
    let (connection, _events, mut peer) = open(WsOptions::default()).await;
    // More than the channel holds, unread, then a 4001.
    for n in 0..200 {
        say(&mut peer, &format!("r{n}"), None, custom()).await;
    }
    peer.close(Some(CloseFrame {
        code: CloseCode::from(4001),
        reason: "".into(),
    }))
    .await
    .unwrap();
    for _ in 0..3 {
        let result = tokio::time::timeout(Duration::from_secs(5), connection.request(subscribe()))
            .await
            .expect("no hang");
        assert!(matches!(
            result,
            Err(RequestError::Closed(CloseReason::AuthFailed))
        ));
    }
}

#[tokio::test]
async fn an_overrun_tells_the_provider_and_lets_the_socket_go() {
    let options = WsOptions {
        event_buffer: 10,
        ..WsOptions::default()
    };
    let (_connection, _events, mut peer) = open(options).await;
    flood(&mut peer, 200).await;
    // The reader has read nothing, and the close still comes.
    let frame = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match peer.next().await {
                Some(Ok(Message::Close(frame))) => return frame,
                Some(Ok(_)) => {}
                other => panic!("no close frame: {other:?}"),
            }
        }
    })
    .await
    .expect("a close frame");
    assert_eq!(frame.unwrap().code, CloseCode::Policy);
}

/// A socket whose writes stop when told to: a provider that has stopped
/// reading, with its receive buffer full.
struct Stuck {
    inner: DuplexStream,
    stuck: Arc<AtomicBool>,
}

impl tokio::io::AsyncRead for Stuck {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for Stuck {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.stuck.load(Ordering::SeqCst) {
            return Poll::Pending;
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.stuck.load(Ordering::SeqCst) {
            return Poll::Pending;
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn an_overrun_fails_requests_before_telling_a_provider_that_has_stopped_reading() {
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let stuck = Arc::new(AtomicBool::new(false));
    let ours = Stuck {
        inner: ours,
        stuck: stuck.clone(),
    };
    let ours = WebSocketStream::from_raw_socket(ours, Role::Client, None).await;
    let mut peer = WebSocketStream::from_raw_socket(theirs, Role::Server, None).await;
    let authority = Authority::canonicalize("a.example").unwrap();
    let parts = Parts {
        actor: Actor::new("ann", authority.clone()).unwrap(),
        authority,
        clock: ProviderClock::new(),
    };
    let options = WsOptions {
        event_buffer: 10,
        ..WsOptions::default()
    };
    let (connection, _events) = spawn(ours, parts, options, Ids::default());

    // One request in flight, then the provider stops reading.
    let held = connection.clone();
    let ask = tokio::spawn(async move { held.request(subscribe()).await });
    command(&mut peer).await;
    stuck.store(true, Ordering::SeqCst);
    flood(&mut peer, 200).await;

    // The policy close cannot be written, and the requests do not wait on it.
    let result = tokio::time::timeout(Duration::from_secs(5), ask)
        .await
        .expect("the request in flight fails at once")
        .unwrap();
    assert!(matches!(
        result,
        Err(RequestError::Closed(CloseReason::Overrun))
    ));
    let result = tokio::time::timeout(Duration::from_secs(5), connection.request(subscribe()))
        .await
        .expect("a new request fails at once");
    assert!(matches!(
        result,
        Err(RequestError::Closed(CloseReason::Overrun))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_request_racing_the_end_is_left_waiting() {
    for _ in 0..20 {
        let (connection, _events, mut peer) = open(WsOptions::default()).await;
        let askers: Vec<_> = (0..16)
            .map(|_| {
                let connection = connection.clone();
                tokio::spawn(async move {
                    loop {
                        if connection
                            .request(subscribe())
                            .await
                            .is_err_and(|e| matches!(e, RequestError::Closed(_)))
                        {
                            return;
                        }
                    }
                })
            })
            .collect();
        // Answer nothing; end the connection while the askers keep sending.
        tokio::time::sleep(Duration::from_millis(5)).await;
        peer.close(Some(CloseFrame {
            code: CloseCode::from(4001),
            reason: "".into(),
        }))
        .await
        .unwrap();
        for asker in askers {
            tokio::time::timeout(Duration::from_secs(5), asker)
                .await
                .expect("every request ends")
                .unwrap();
        }
    }
}
