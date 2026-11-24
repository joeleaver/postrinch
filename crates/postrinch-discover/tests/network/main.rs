//! The real prober against scripted servers on loopback, and the real
//! fetcher's refusal of anything but HTTPS.

mod tls;

use std::time::Duration;

use postrinch_discover::{
    Fetch, FetchError, HttpsFetch, NetProbe, Probe, ProbeError, Protocol, Security,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use wiremock::{Mock, MockServer, ResponseTemplate};

use tls::{Authority, Ended, starttls_server};

/// A server on loopback that sends `greeting`, then answers each line it
/// reads with the first reply whose prefix the line starts with. Returns
/// its port.
async fn scripted(greeting: &'static str, replies: &'static [(&'static str, &'static str)]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        write.write_all(greeting.as_bytes()).await.unwrap();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let upper = line.to_ascii_uppercase();
            match replies.iter().find(|(prefix, _)| upper.starts_with(prefix)) {
                Some((_, reply)) => {
                    if write.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
                None => return,
            }
        }
    });
    port
}

async fn probe(protocol: Protocol, port: u16, security: Security) -> Result<(), ProbeError> {
    probe_with(NetProbe::new().unwrap(), protocol, port, security).await
}

async fn probe_with(
    prober: NetProbe,
    protocol: Protocol,
    port: u16,
    security: Security,
) -> Result<(), ProbeError> {
    tokio::time::timeout(
        Duration::from_secs(5),
        prober.probe(protocol, "127.0.0.1", port, security),
    )
    .await
    .expect("the probe finished")
}

/// Probes for STARTTLS on `port` trusting `authority`'s root, and says how
/// the server's session ended.
async fn probe_starttls(
    authority: &Authority,
    protocol: Protocol,
    port: u16,
    session: tokio::task::JoinHandle<Ended>,
) -> (Result<(), ProbeError>, Ended) {
    let prober = NetProbe::with_tls_config(authority.client());
    let answer = probe_with(prober, protocol, port, Security::StartTls).await;
    (answer, session.await.unwrap())
}

#[tokio::test]
async fn imap_starttls_in_the_greeting() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "* OK [CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED] ready\r\n",
        &[],
        "P2 STARTTLS",
        "P2 OK begin TLS\r\n",
        ca.server(&["127.0.0.1"]),
    )
    .await;
    assert_eq!(
        probe_starttls(&ca, Protocol::Imap, port, session).await,
        (Ok(()), Ended::OverTls("P3 LOGOUT".to_owned()))
    );
}

#[tokio::test]
async fn imap_starttls_from_capability() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "* OK ready\r\n",
        &[(
            "P1 CAPABILITY",
            "* CAPABILITY IMAP4rev1 starttls AUTH=PLAIN\r\nP1 OK done\r\n",
        )],
        "P2 STARTTLS",
        "P2 OK begin TLS\r\n",
        ca.server(&["127.0.0.1"]),
    )
    .await;
    assert_eq!(
        probe_starttls(&ca, Protocol::Imap, port, session).await,
        (Ok(()), Ended::OverTls("P3 LOGOUT".to_owned()))
    );
}

#[tokio::test]
async fn imap_starttls_with_a_certificate_for_another_name_is_refused() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "* OK [CAPABILITY IMAP4rev1 STARTTLS] ready\r\n",
        &[],
        "P2 STARTTLS",
        "P2 OK begin TLS\r\n",
        ca.server(&["imap.example.org"]),
    )
    .await;
    let (answer, ended) = probe_starttls(&ca, Protocol::Imap, port, session).await;
    assert!(matches!(answer, Err(ProbeError::Connect(_))), "{answer:?}");
    assert_eq!(ended, Ended::HandshakeFailed);
}

#[tokio::test]
async fn starttls_with_an_untrusted_root_is_refused() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "* OK [CAPABILITY IMAP4rev1 STARTTLS] ready\r\n",
        &[],
        "P2 STARTTLS",
        "P2 OK begin TLS\r\n",
        ca.server(&["127.0.0.1"]),
    )
    .await;
    let answer = probe(Protocol::Imap, port, Security::StartTls).await;
    assert!(matches!(answer, Err(ProbeError::Connect(_))), "{answer:?}");
    assert_eq!(session.await.unwrap(), Ended::HandshakeFailed);
}

#[tokio::test]
async fn imap_preauth_on_a_starttls_port_is_refused() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "* PREAUTH [CAPABILITY IMAP4rev1 STARTTLS] signed in\r\n",
        &[],
        "P2 STARTTLS",
        "P2 OK begin TLS\r\n",
        ca.server(&["127.0.0.1"]),
    )
    .await;
    assert_eq!(
        probe_starttls(&ca, Protocol::Imap, port, session).await,
        (Err(ProbeError::PreAuth), Ended::NoStartTls)
    );
}

#[tokio::test]
async fn imap_starttls_refused_after_being_offered() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "* OK [CAPABILITY IMAP4rev1 STARTTLS] ready\r\n",
        &[],
        "P2 STARTTLS",
        "P2 NO not today\r\n",
        ca.server(&["127.0.0.1"]),
    )
    .await;
    let (answer, _) = probe_starttls(&ca, Protocol::Imap, port, session).await;
    assert_eq!(
        answer,
        Err(ProbeError::StartTlsRefused("P2 NO not today".to_owned()))
    );
}

#[tokio::test]
async fn data_sent_before_tls_begins_is_refused() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "* OK [CAPABILITY IMAP4rev1 STARTTLS] ready\r\n",
        &[],
        "P2 STARTTLS",
        "P2 OK begin TLS\r\n* OK injected\r\n",
        ca.server(&["127.0.0.1"]),
    )
    .await;
    let (answer, _) = probe_starttls(&ca, Protocol::Imap, port, session).await;
    assert!(matches!(answer, Err(ProbeError::Io(_))), "{answer:?}");
}

#[tokio::test]
async fn imap_without_starttls() {
    let port = scripted(
        "* OK ready\r\n",
        &[(
            "P1 CAPABILITY",
            "* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\nP1 OK done\r\n",
        )],
    )
    .await;
    assert_eq!(
        probe(Protocol::Imap, port, Security::StartTls).await,
        Err(ProbeError::NoStartTls)
    );
}

#[tokio::test]
async fn smtp_starttls_from_ehlo() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "220-mail.example.org ESMTP\r\n220 welcome\r\n",
        &[(
            "EHLO [127.0.0.1]",
            "250-mail.example.org\r\n250-PIPELINING\r\n250-STARTTLS\r\n250 8BITMIME\r\n",
        )],
        "STARTTLS",
        "220 go ahead\r\n",
        ca.server(&["127.0.0.1"]),
    )
    .await;
    assert_eq!(
        probe_starttls(&ca, Protocol::Smtp, port, session).await,
        (Ok(()), Ended::OverTls("QUIT".to_owned()))
    );
}

#[tokio::test]
async fn smtp_starttls_with_a_certificate_for_another_name_is_refused() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "220 mail.example.org ESMTP\r\n",
        &[("EHLO", "250-mail.example.org\r\n250 STARTTLS\r\n")],
        "STARTTLS",
        "220 go ahead\r\n",
        ca.server(&["smtp.example.org"]),
    )
    .await;
    let (answer, ended) = probe_starttls(&ca, Protocol::Smtp, port, session).await;
    assert!(matches!(answer, Err(ProbeError::Connect(_))), "{answer:?}");
    assert_eq!(ended, Ended::HandshakeFailed);
}

#[tokio::test]
async fn smtp_starttls_refused_after_being_offered() {
    let ca = Authority::new();
    let (port, session) = starttls_server(
        "220 mail.example.org ESMTP\r\n",
        &[("EHLO", "250-mail.example.org\r\n250 STARTTLS\r\n")],
        "STARTTLS",
        "454 TLS not available\r\n",
        ca.server(&["127.0.0.1"]),
    )
    .await;
    let (answer, _) = probe_starttls(&ca, Protocol::Smtp, port, session).await;
    assert_eq!(
        answer,
        Err(ProbeError::StartTlsRefused(
            "454 TLS not available".to_owned()
        ))
    );
}

#[tokio::test]
async fn smtp_without_starttls() {
    let port = scripted(
        "220 mail.example.org ESMTP\r\n",
        &[(
            "EHLO",
            "250-mail.example.org\r\n250 AUTH PLAIN STARTTLS\r\n",
        )],
    )
    .await;
    assert_eq!(
        probe(Protocol::Smtp, port, Security::StartTls).await,
        Err(ProbeError::NoStartTls)
    );
}

#[tokio::test]
async fn the_wrong_protocol_is_refused() {
    let port = scripted("220 mail.example.org ESMTP\r\n", &[]).await;
    assert!(matches!(
        probe(Protocol::Imap, port, Security::StartTls).await,
        Err(ProbeError::Greeting(_))
    ));
    let port = scripted("* OK ready\r\n", &[]).await;
    assert!(matches!(
        probe(Protocol::Smtp, port, Security::StartTls).await,
        Err(ProbeError::Greeting(_))
    ));
}

#[tokio::test]
async fn tls_against_a_plain_server_fails() {
    let port = scripted("* OK ready\r\n", &[]).await;
    assert!(matches!(
        probe(Protocol::Imap, port, Security::Tls).await,
        Err(ProbeError::Connect(_))
    ));
}

#[tokio::test]
async fn nothing_listening() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    assert!(matches!(
        probe(Protocol::Imap, port, Security::Tls).await,
        Err(ProbeError::Connect(_))
    ));
}

#[tokio::test]
async fn https_fetch_refuses_plain_http() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_string("<clientConfig/>"))
        .mount(&server)
        .await;
    let fetch = HttpsFetch::new(Duration::from_secs(5)).unwrap();
    let url = format!("{}/mail/config-v1.1.xml", server.uri());

    assert!(matches!(
        fetch.get(&url).await,
        Err(FetchError::NotHttps(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}
