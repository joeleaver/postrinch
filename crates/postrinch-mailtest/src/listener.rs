//! Accepting connections on a loopback port, for both fakes.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;

use crate::Error;
use crate::conn::Conn;

/// Listens on `127.0.0.1` on a free port and serves each connection with
/// `serve`, after a TLS handshake when `tls` is set. Returns the address
/// and the listening task; aborting the task stops the listener and
/// every session it started.
pub(crate) async fn listen<F, Fut>(
    acceptor: TlsAcceptor,
    tls: bool,
    serve: F,
) -> Result<(SocketAddr, JoinHandle<()>), Error>
where
    F: Fn(Conn) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let serve = Arc::new(serve);
    let task = tokio::spawn(async move {
        // The sessions belong to this task, so they end with it.
        let mut sessions = JoinSet::new();
        loop {
            let socket = match listener.accept().await {
                Ok((socket, _)) => socket,
                // A connection that failed before it was accepted (reset,
                // aborted) is the client's to report; the listener goes on.
                Err(_) => continue,
            };
            while sessions.try_join_next().is_some() {}
            let serve = serve.clone();
            let acceptor = acceptor.clone();
            // The handshake runs in the session's own task, so a slow
            // client cannot hold up the listener.
            sessions.spawn(async move {
                let conn = if tls {
                    match acceptor.accept(socket).await {
                        Ok(stream) => Conn::new(Box::new(stream), true),
                        // A failed handshake is the client's to report.
                        Err(_) => return,
                    }
                } else {
                    Conn::new(Box::new(socket), false)
                };
                serve(conn).await;
            });
        }
    });
    Ok((addr, task))
}
