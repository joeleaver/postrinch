//! Connecting to a server the way RFC 8305 ("Happy Eyeballs") does: every
//! address the name resolves to, IPv6 and IPv4 taking turns, a new attempt
//! started every [`ATTEMPT_DELAY`] (or as soon as one fails) while the
//! earlier ones keep going, and the first to connect wins. An address that
//! silently drops packets then costs one delay, not the whole probe.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::task::JoinSet;

/// How long an attempt runs alone before the next address is tried as well
/// (RFC 8305 §5 recommends 250 ms).
pub(crate) const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// Resolves `host` and connects to the first of its addresses that answers.
/// The overall time limit is the caller's.
pub(crate) async fn connect(host: &str, port: u16) -> io::Result<TcpStream> {
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
    race(addresses, ATTEMPT_DELAY, TcpStream::connect).await
}

/// The order addresses are tried in: the resolver's order within each
/// family, the families taking turns, starting with the family of the
/// resolver's first answer (RFC 8305 §4).
pub(crate) fn interleave(addresses: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let first_is_v6 = addresses.first().is_some_and(SocketAddr::is_ipv6);
    let (preferred, other): (Vec<SocketAddr>, Vec<SocketAddr>) = addresses
        .into_iter()
        .partition(|a| a.is_ipv6() == first_is_v6);
    let mut out = Vec::with_capacity(preferred.len() + other.len());
    let mut preferred = preferred.into_iter();
    let mut other = other.into_iter();
    loop {
        match (preferred.next(), other.next()) {
            (None, None) => return out,
            (a, b) => out.extend(a.into_iter().chain(b)),
        }
    }
}

/// Races connection attempts to `addresses` (in [`interleave`] order),
/// starting one every `delay`, or at once when the latest fails. The first
/// success wins and the attempts still running are dropped; when every
/// attempt fails, the last failure is the answer.
pub(crate) async fn race<C, F, Fut>(
    addresses: Vec<SocketAddr>,
    delay: Duration,
    connect: F,
) -> io::Result<C>
where
    C: Send + 'static,
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = io::Result<C>> + Send + 'static,
{
    let mut waiting = interleave(addresses).into_iter();
    let mut running = JoinSet::new();
    let mut last_error = None;
    loop {
        if let Some(address) = waiting.next() {
            running.spawn(connect(address));
        } else if running.is_empty() {
            break;
        }
        // Wait for an attempt to end, or for the delay when there is
        // another address to start.
        let ended = if waiting.len() > 0 {
            match tokio::time::timeout(delay, running.join_next()).await {
                Ok(ended) => ended,
                Err(_) => continue,
            }
        } else {
            running.join_next().await
        };
        match ended {
            Some(Ok(Ok(connection))) => return Ok(connection),
            Some(Ok(Err(e))) => last_error = Some(e),
            Some(Err(e)) => last_error = Some(io::Error::other(e)),
            None => {}
        }
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the name has no addresses")))
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use tokio::time::Instant;

    use super::*;

    const fn v4(last: u8) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, last), 993))
    }

    fn v6(last: u16) -> SocketAddr {
        SocketAddr::from(([0x2001, 0xdb8, 0, 0, 0, 0, 0, last], 993))
    }

    /// A fake connection attempt: it connects to the address it answers.
    type Attempt = Pin<Box<dyn Future<Output = io::Result<SocketAddr>> + Send>>;

    /// A connector that records each address it is asked for. IPv6
    /// addresses never answer (a black hole); IPv4 ones in `refused` fail
    /// at once, and the other IPv4 ones connect.
    fn connector(
        refused: &'static [SocketAddr],
    ) -> (Arc<Mutex<Vec<SocketAddr>>>, impl Fn(SocketAddr) -> Attempt) {
        let tried = Arc::new(Mutex::new(Vec::new()));
        let log = tried.clone();
        let connect = move |address: SocketAddr| {
            log.lock().unwrap().push(address);
            Box::pin(async move {
                if address.is_ipv6() {
                    std::future::pending::<()>().await;
                }
                if refused.contains(&address) {
                    Err(io::Error::from(io::ErrorKind::ConnectionRefused))
                } else {
                    Ok(address)
                }
            }) as Attempt
        };
        (tried, connect)
    }

    #[test]
    fn families_take_turns_starting_with_the_first() {
        assert_eq!(
            interleave(vec![v6(1), v6(2), v6(3), v4(1), v4(2)]),
            [v6(1), v4(1), v6(2), v4(2), v6(3)]
        );
        assert_eq!(interleave(vec![v4(1), v4(2), v6(1)]), [v4(1), v6(1), v4(2)]);
        assert_eq!(interleave(vec![v4(1), v4(2)]), [v4(1), v4(2)]);
        assert_eq!(interleave(Vec::new()), []);
    }

    #[tokio::test(start_paused = true)]
    async fn a_black_holed_ipv6_address_costs_one_delay() {
        let (tried, connect) = connector(&[]);
        let start = Instant::now();
        let won = race(vec![v6(1), v6(2), v4(1)], ATTEMPT_DELAY, connect).await;
        assert_eq!(won.unwrap(), v4(1));
        assert_eq!(start.elapsed(), ATTEMPT_DELAY);
        assert_eq!(*tried.lock().unwrap(), [v6(1), v4(1)]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_starts_the_next_attempt_at_once() {
        static REFUSED: [SocketAddr; 1] = [v4(1)];
        let (tried, connect) = connector(&REFUSED);
        let start = Instant::now();
        let won = race(vec![v4(1), v4(2)], ATTEMPT_DELAY, connect).await;
        assert_eq!(won.unwrap(), v4(2));
        assert_eq!(start.elapsed(), Duration::ZERO);
        assert_eq!(*tried.lock().unwrap(), [v4(1), v4(2)]);
    }

    #[tokio::test(start_paused = true)]
    async fn every_attempt_failing_answers_the_last_failure() {
        static REFUSED: [SocketAddr; 2] = [v4(1), v4(2)];
        let (_, connect) = connector(&REFUSED);
        let failed = race(vec![v4(1), v4(2)], ATTEMPT_DELAY, connect).await;
        assert_eq!(failed.unwrap_err().kind(), io::ErrorKind::ConnectionRefused);
        let (_, connect) = connector(&[]);
        let failed = race(Vec::new(), ATTEMPT_DELAY, connect).await;
        assert_eq!(failed.unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn connects_to_loopback_by_name() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let stream = connect("127.0.0.1", port).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap().port(), port);
    }
}
