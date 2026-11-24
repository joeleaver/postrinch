//! The fake SMTP server, driven by lettre (the client Postrinch uses).

use std::net::SocketAddr;

use lettre::address::Envelope;
use lettre::transport::smtp::Error as SmtpError;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Certificate, Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use postrinch_mailtest::{HOST, SmtpConfig, SmtpFault, SmtpServer, SmtpStage};

const MAIL: &[u8] =
    b"From: bob@example.com\r\nTo: ann@example.com\r\nSubject: Hi\r\n\r\nHello\r\n.leading dot\r\n";

async fn server() -> SmtpServer {
    SmtpServer::start(
        SmtpConfig::new()
            .user("bob", "hunter2")
            .oauth_token("bob", "tok"),
    )
    .await
    .unwrap()
}

enum Port {
    Implicit,
    StartTls,
}

fn transport(
    server: &SmtpServer,
    port: Port,
    user: &str,
    secret: &str,
    mechanism: Mechanism,
) -> AsyncSmtpTransport<Tokio1Executor> {
    let cert = Certificate::from_der(server.certificate().der().to_vec()).unwrap();
    let tls = TlsParameters::builder(HOST.to_string())
        .add_root_certificate(cert)
        .build_rustls()
        .unwrap();
    let (addr, tls): (SocketAddr, Tls) = match port {
        Port::Implicit => (server.tls_addr().unwrap(), Tls::Wrapper(tls)),
        Port::StartTls => (server.starttls_addr().unwrap(), Tls::Required(tls)),
    };
    AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(addr.ip().to_string())
        .port(addr.port())
        .tls(tls)
        .credentials(Credentials::new(user.to_string(), secret.to_string()))
        .authentication(vec![mechanism])
        .build()
}

fn envelope() -> Envelope {
    Envelope::new(
        Some("bob@example.com".parse().unwrap()),
        vec![
            "ann@example.com".parse().unwrap(),
            "cy@example.com".parse().unwrap(),
        ],
    )
    .unwrap()
}

async fn send(transport: &AsyncSmtpTransport<Tokio1Executor>) -> Result<(), SmtpError> {
    transport.send_raw(&envelope(), MAIL).await.map(|_| ())
}

#[tokio::test]
async fn implicit_tls_with_plain() {
    let server = server().await;
    let t = transport(&server, Port::Implicit, "bob", "hunter2", Mechanism::Plain);
    send(&t).await.unwrap();
    let messages = server.messages();
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.auth_user.as_deref(), Some("bob"));
    assert_eq!(message.mail_from, "bob@example.com");
    assert_eq!(message.rcpt_to, ["ann@example.com", "cy@example.com"]);
    // Dot-stuffed by lettre, unstuffed by the server: the bytes survive.
    assert_eq!(message.data, MAIL);
}

#[tokio::test]
async fn starttls_with_login_and_xoauth2() {
    let server = server().await;
    let t = transport(&server, Port::StartTls, "bob", "hunter2", Mechanism::Login);
    send(&t).await.unwrap();
    let t = transport(&server, Port::StartTls, "bob", "tok", Mechanism::Xoauth2);
    send(&t).await.unwrap();
    assert_eq!(server.messages().len(), 2);
    assert_eq!(server.command_count("STARTTLS"), 2);
    // EHLO before and after STARTTLS, on each connection.
    assert_eq!(server.command_count("EHLO"), 4);
    assert!(server.commands().contains(&"AUTH LOGIN".to_string()));
}

#[tokio::test]
async fn wrong_credentials_are_refused() {
    let server = server().await;
    let t = transport(&server, Port::Implicit, "bob", "nope", Mechanism::Plain);
    let error = send(&t).await.unwrap_err();
    assert!(error.is_permanent(), "{error}");
    assert_eq!(
        error.status().map(|c| c.to_string()),
        Some("535".to_string())
    );
    let t = transport(&server, Port::Implicit, "bob", "nope", Mechanism::Xoauth2);
    assert!(send(&t).await.is_err());
    assert!(server.messages().is_empty());
}

#[tokio::test]
async fn programmed_replies() {
    let server = server().await;
    let t = transport(&server, Port::Implicit, "bob", "hunter2", Mechanism::Plain);

    server.fail_next(
        SmtpStage::Rcpt,
        1,
        SmtpFault::Reply {
            code: 450,
            text: "4.2.1 Mailbox busy".into(),
        },
    );
    let error = send(&t).await.unwrap_err();
    assert!(error.is_transient(), "{error}");

    server.fail_next(
        SmtpStage::Mail,
        1,
        SmtpFault::Reply {
            code: 550,
            text: "5.7.1 Sender refused".into(),
        },
    );
    let error = send(&t).await.unwrap_err();
    assert!(error.is_permanent(), "{error}");

    server.fail_next(
        SmtpStage::Data,
        1,
        SmtpFault::Reply {
            code: 554,
            text: "5.6.0 Content refused".into(),
        },
    );
    assert!(send(&t).await.unwrap_err().is_permanent());

    server.fail_next(
        SmtpStage::Auth,
        1,
        SmtpFault::Reply {
            code: 454,
            text: "4.7.0 Try later".into(),
        },
    );
    assert!(send(&t).await.unwrap_err().is_transient());

    assert!(server.messages().is_empty());
    send(&t).await.unwrap();
    assert_eq!(server.messages().len(), 1);
}

#[tokio::test]
async fn drop_after_data_before_storing_leaves_the_client_unsure() {
    let server = server().await;
    let t = transport(&server, Port::Implicit, "bob", "hunter2", Mechanism::Plain);
    server.fail_next(SmtpStage::Data, 1, SmtpFault::Disconnect);
    let error = send(&t).await.unwrap_err();
    assert!(!error.is_permanent() && !error.is_transient(), "{error}");
    assert!(server.messages().is_empty());
    assert_eq!(server.command_count("DATA"), 1);
    // A retry delivers it once.
    send(&t).await.unwrap();
    assert_eq!(server.messages().len(), 1);
}

#[tokio::test]
async fn drop_after_storing_makes_a_retry_a_duplicate() {
    let server = server().await;
    let t = transport(&server, Port::Implicit, "bob", "hunter2", Mechanism::Plain);
    server.fail_next(SmtpStage::Data, 1, SmtpFault::StoreThenDisconnect);
    let error = send(&t).await.unwrap_err();
    // The client sees exactly what it sees when nothing was stored.
    assert!(!error.is_permanent() && !error.is_transient(), "{error}");
    assert_eq!(server.messages().len(), 1);
    assert_eq!(server.messages()[0].data, MAIL);
    send(&t).await.unwrap();
    assert_eq!(server.messages().len(), 2);
}

#[test]
#[should_panic(expected = "StoreThenDisconnect only applies at SmtpStage::Data")]
fn store_then_disconnect_needs_the_data_stage() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(server());
    server.fail_next(SmtpStage::Rcpt, 1, SmtpFault::StoreThenDisconnect);
}

#[tokio::test]
async fn data_is_kept_exactly_and_malformed_data_is_refused() {
    let server = server().await;
    let t = transport(&server, Port::Implicit, "bob", "hunter2", Mechanism::Plain);
    // Trailing spaces, an empty last line, 8-bit bytes: all kept.
    let exact: &[u8] = b"Subject: x  \r\n\r\nCaf\xc3\xa9 \t\r\n..two dots\r\n\r\n";
    t.send_raw(&envelope(), exact).await.unwrap();
    assert_eq!(server.messages()[0].data, exact);

    // RFC 5321 4.5.3.1.6: at most 1000 octets a line, CRLF included.
    let mut long = b"Subject: long\r\n\r\n".to_vec();
    long.extend(std::iter::repeat_n(b'a', 999));
    long.extend_from_slice(b"\r\n");
    let error = t.send_raw(&envelope(), &long).await.unwrap_err();
    assert!(error.is_permanent(), "{error}");
    assert_eq!(server.messages().len(), 1);
}
