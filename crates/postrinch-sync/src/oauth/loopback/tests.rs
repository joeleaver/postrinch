use std::net::SocketAddr;

use super::*;

/// Long enough for a test to fail rather than hang, and shorter than
/// [`CONNECTION_TIMEOUT`]: a wait that finishes within it was not held
/// up by a connection that sent nothing.
const PROMPT: Duration = Duration::from_secs(3);

#[test]
fn classifies_redirects() {
    let ours = |target| redirect_result(target, "s", "Google");
    assert_eq!(ours("/?state=s&code=abc"), Redirect::Code("abc".into()));
    assert_eq!(ours("/?state=other&code=abc"), Redirect::NotOurs);
    assert_eq!(ours("/favicon.ico"), Redirect::NotOurs);
    assert_eq!(
        ours("/?state=s&error=access_denied"),
        Redirect::Error("access was not granted".into())
    );
    assert!(matches!(ours("/?state=s"), Redirect::Error(_)));
}

#[test]
fn a_refusal_without_words_names_the_provider() {
    assert_eq!(
        redirect_result("/?state=s&error=server_error", "s", "Google"),
        Redirect::Error("Google answered server_error".into())
    );
    assert_eq!(
        redirect_result("/?state=s&error=temporarily_unavailable", "s", "Microsoft"),
        Redirect::Error("Microsoft answered temporarily_unavailable".into())
    );
}

#[test]
fn a_refusal_keeps_the_providers_words() {
    let target = "/?state=s&error=consent_required\
        &error_description=AADSTS65001%3A+Not+consented.%0D%0ATrace+ID%3A+x";
    assert_eq!(
        redirect_result(target, "s", "Microsoft"),
        Redirect::Error("AADSTS65001: Not consented.".into())
    );
}

/// The port a redirect URI names.
fn port(uri: &str) -> u16 {
    let url = Url::parse(uri).unwrap();
    url.port().unwrap()
}

/// Whether this host has an IPv6 loopback to listen on.
fn has_ipv6_loopback() -> bool {
    std::net::TcpListener::bind(("::1", 0)).is_ok()
}

async fn redirect(to: SocketAddr, query: &str) -> reqwest::StatusCode {
    let url = match to {
        SocketAddr::V4(a) => format!("http://{a}/?{query}"),
        SocketAddr::V6(a) => format!("http://[{}]:{}/?{query}", a.ip(), a.port()),
    };
    reqwest::get(url).await.unwrap().status()
}

fn v4(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn v6(port: u16) -> SocketAddr {
    SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port))
}

#[tokio::test]
async fn receives_the_code_over_http() {
    let loopback = Loopback::bind(RedirectHost::Ip).await.unwrap();
    let uri = loopback.redirect_uri().to_owned();
    let wait = tokio::spawn(loopback.wait_for_code("st", "Google", PROMPT));

    let http = reqwest::Client::new();
    let favicon = http.get(format!("{uri}favicon.ico")).send().await.unwrap();
    assert_eq!(favicon.status(), 404);
    assert_eq!(
        redirect(v4(port(&uri)), "state=st&code=the-code").await,
        200
    );
    assert_eq!(wait.await.unwrap().unwrap(), "the-code");
}

#[tokio::test]
async fn an_idle_connection_does_not_hold_up_the_redirect() {
    let loopback = Loopback::bind(RedirectHost::Ip).await.unwrap();
    let port = port(loopback.redirect_uri());
    let wait = tokio::spawn(loopback.wait_for_code("st", "Google", PROMPT));

    // A browser's preconnect: open, and say nothing.
    let _idle = TcpStream::connect(v4(port)).await.unwrap();
    assert_eq!(redirect(v4(port), "state=st&code=the-code").await, 200);
    assert_eq!(wait.await.unwrap().unwrap(), "the-code");
}

#[tokio::test]
async fn garbage_and_resets_do_not_end_the_wait() {
    let loopback = Loopback::bind(RedirectHost::Ip).await.unwrap();
    let port = port(loopback.redirect_uri());
    let wait = tokio::spawn(loopback.wait_for_code("st", "Google", PROMPT));

    let mut garbage = TcpStream::connect(v4(port)).await.unwrap();
    garbage
        .write_all(b"\x16\x03\x01 not http at all\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    garbage.read_to_end(&mut answer).await.unwrap();
    assert!(answer.starts_with(b"HTTP/1.1 400"), "{answer:?}");

    // Half a request, then a reset.
    let reset = TcpStream::connect(v4(port)).await.unwrap();
    let mut reset = reset;
    reset.write_all(b"GET /?state=st").await.unwrap();
    reset.set_zero_linger().unwrap();
    drop(reset);

    // A request too long to be a redirect.
    let mut long = TcpStream::connect(v4(port)).await.unwrap();
    let flood = vec![b'a'; MAX_REQUEST + 4096];
    let _ = long.write_all(&flood).await;

    assert_eq!(redirect(v4(port), "state=st&code=the-code").await, 200);
    assert_eq!(wait.await.unwrap().unwrap(), "the-code");
}

#[tokio::test]
async fn the_wait_ends_on_the_first_redirect_and_closes_the_listener() {
    let loopback = Loopback::bind(RedirectHost::Ip).await.unwrap();
    let port = port(loopback.redirect_uri());
    let wait = tokio::spawn(loopback.wait_for_code("st", "Google", PROMPT));
    assert_eq!(redirect(v4(port), "state=st&code=first").await, 200);
    assert_eq!(wait.await.unwrap().unwrap(), "first");
    assert!(TcpStream::connect(v4(port)).await.is_err());
}

#[tokio::test]
async fn a_localhost_redirect_is_heard_on_127_0_0_1() {
    let loopback = Loopback::bind(RedirectHost::Localhost).await.unwrap();
    let uri = loopback.redirect_uri().to_owned();
    assert!(uri.starts_with("http://localhost:"), "{uri}");
    let port = port(&uri);
    let has_v6 = loopback.v6.is_some();
    let wait = tokio::spawn(loopback.wait_for_code("st", "Microsoft", PROMPT));

    // The browser tried ::1 first and gave up on it without a word.
    let _idle = if has_v6 {
        Some(TcpStream::connect(v6(port)).await.unwrap())
    } else {
        None
    };
    assert_eq!(redirect(v4(port), "state=st&code=the-code").await, 200);
    assert_eq!(wait.await.unwrap().unwrap(), "the-code");
}

#[tokio::test]
async fn a_localhost_redirect_is_heard_on_ipv6_loopback() {
    if !has_ipv6_loopback() {
        eprintln!("no IPv6 loopback on this host: only 127.0.0.1 is listened on");
        return;
    }
    let loopback = Loopback::bind(RedirectHost::Localhost).await.unwrap();
    assert!(loopback.v6.is_some(), "::1 is up but not listened on");
    let port = port(loopback.redirect_uri());
    let wait = tokio::spawn(loopback.wait_for_code("st", "Microsoft", PROMPT));

    // A stale tab's redirect, and an idle connection on 127.0.0.1.
    assert_eq!(redirect(v6(port), "state=old&code=x").await, 404);
    let _idle = TcpStream::connect(v4(port)).await.unwrap();
    assert_eq!(redirect(v6(port), "state=st&code=the-code").await, 200);
    assert_eq!(wait.await.unwrap().unwrap(), "the-code");
}
