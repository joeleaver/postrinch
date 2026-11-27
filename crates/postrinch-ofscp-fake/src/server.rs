//! The accept loop: TCP, then TLS, then HTTP/1.1 with upgrades, one task
//! per connection, each of which a fault can drop.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::Request;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt as _;

/// Drops the connection a request arrived on, with no TLS close_notify
/// (inserted into every request's extensions).
#[derive(Clone)]
pub(crate) struct ConnKill(Arc<Notify>);

impl ConnKill {
    /// Drop the connection. The caller should then wait forever: the
    /// connection's task, its own included, ends.
    pub(crate) fn kill(&self) {
        self.0.notify_one();
    }
}

/// The open connections, so a fault can drop them all.
#[derive(Debug, Default)]
pub(crate) struct ConnRegistry {
    open: Mutex<HashMap<u64, Arc<Notify>>>,
    next: AtomicU64,
}

impl ConnRegistry {
    fn add(&self) -> (u64, Arc<Notify>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let kill = Arc::new(Notify::new());
        self.open.lock().unwrap().insert(id, kill.clone());
        (id, kill)
    }

    fn remove(&self, id: u64) {
        self.open.lock().unwrap().remove(&id);
    }

    pub(crate) fn abort_all(&self) {
        for kill in self.open.lock().unwrap().values() {
            kill.notify_one();
        }
    }
}

/// Serve `router` on `listener` until the task is aborted.
pub(crate) async fn serve(listener: TcpListener, router: Router, conns: Arc<ConnRegistry>) {
    let acceptor = TlsAcceptor::from(crate::tls::server_config());
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            continue;
        };
        let acceptor = acceptor.clone();
        let router = router.clone();
        let conns = conns.clone();
        tokio::spawn(async move {
            let (id, kill) = conns.add();
            let tag = ConnKill(kill.clone());
            let service = router.map_request(move |mut req: Request<hyper::body::Incoming>| {
                req.extensions_mut().insert(tag.clone());
                req
            });
            let connection = async {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), TowerToHyperService::new(service))
                    .with_upgrades()
                    .await;
            };
            tokio::select! {
                () = connection => {}
                () = kill.notified() => {}
            }
            conns.remove(id);
        });
    }
}
