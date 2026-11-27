//! Opt-in interop against a real OFSCP provider (Forumall, re-ofscp).
//!
//! Ignored by default. Local providers serve plain HTTP, and this client
//! speaks only HTTPS and WSS, so the test puts a TLS proxy in front of the
//! provider with a certificate it trusts:
//!
//! ```text
//! OFSCP_INTEROP_UPSTREAM=127.0.0.1:8444   # the provider's plain HTTP port
//! OFSCP_INTEROP_DOMAIN=localhost:8443     # its DOMAIN; the proxy listens here
//! cargo test -p postrinch-ofscp --test interop -- --ignored --nocapture
//! ```
//!
//! `OFSCP_INTEROP_SKIP=rest-post` leaves out the REST post, for a provider
//! older than it (Forumall posts over the socket only).
//!
//! The provider's `DOMAIN` must be `OFSCP_INTEROP_DOMAIN`, since signatures
//! bind to it (§4.4.2). Registration must be open (§4.1.1).
//!
//! It checks: discovery and its domain, registration, the device key and
//! the signed requests made with it, `GET /api/me`, a group, a channel,
//! the WebSocket challenge sign-in, `subscribe`, `ping`, a
//! `message.create` over the socket and a REST post (both seen as
//! `message.created` and read back in the history), and signing out.

use std::sync::Arc;
use std::time::Duration;

use ofscp_core::wire::{
    ChannelCreateRequest, ChannelType, Content, GroupCreateRequest, MessageCreate,
    RegistrationRequest, TimelineItem,
};
use ofscp_core::ws::{ChannelMessageCreate, Command, Event, Subscribe};
use ofscp_core::{Authority, LocalId};
use postrinch_ofscp::http::PageRequest;
use postrinch_ofscp::ws::{self, Events, Incoming, WsOptions};
use postrinch_ofscp::{Endpoint, Options, Tls, Transport};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; see tests/interop.rs"))
}

/// Terminate TLS on `domain`'s port and pass the bytes to `upstream`.
/// Returns the certificate to trust.
async fn tls_proxy(domain: &Authority, upstream: String) -> Vec<u8> {
    let host = domain
        .as_str()
        .rsplit_once(':')
        .map_or(domain.as_str(), |(h, _)| h);
    let port = domain
        .as_str()
        .rsplit_once(':')
        .map_or(443, |(_, p)| p.parse().expect("a port"));
    let cert = rcgen::generate_simple_self_signed(vec![host.to_owned()]).expect("a certificate");
    let der = cert.cert.der().to_vec();
    let key = rustls::pki_types::PrivateKeyDer::try_from(cert.signing_key.serialize_der())
        .expect("a key");
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![der.clone().into()], key)
        .expect("a server config");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("the proxy port is free");
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let upstream = upstream.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Ok(mut plain) = TcpStream::connect(upstream).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut tls, &mut plain).await;
            });
        }
    });
    der
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running provider; see the module docs"]
async fn a_real_provider_speaks_with_this_client() {
    let domain = Authority::canonicalize(&env("OFSCP_INTEROP_DOMAIN")).expect("an authority");
    let root = tls_proxy(&domain, env("OFSCP_INTEROP_UPSTREAM")).await;
    let transport = Transport::new(
        Tls::with_extra_roots(vec![root]).expect("tls"),
        Options::default(),
    )
    .expect("a transport");
    let provider = transport.provider(Endpoint::new(domain.clone()));

    let discovery = provider.discover().await.expect("discovery");
    println!("discovered {:?}", discovery.provider);

    let handle = format!("interop{}", std::process::id());
    let password = "interop-password-0123456789";
    let bootstrap = provider
        .register(&RegistrationRequest {
            handle: handle.clone(),
            password: password.to_owned(),
            recovery_email: None,
            invite_token: None,
        })
        .await
        .expect("registration");
    let identity = provider
        .register_device(&handle, &bootstrap, "postrinch interop")
        .await
        .expect("a device key");
    let client = transport.client(Arc::new(identity), Endpoint::new(domain.clone()));
    let me = client.me().await.expect("GET /api/me");
    println!("signed in as {me:?}");
    let keys = client.device_keys().await.expect("device keys");
    println!("device keys: {keys:?}");

    let group = client
        .create_group(&GroupCreateRequest {
            name: "Interop".into(),
            description: None,
            tier: None,
            join_policy: None,
            permissions: None,
            roles: None,
            metadata: None,
        })
        .await
        .expect("a group");
    let channel = client
        .create_channel(
            &group.id,
            &ChannelCreateRequest {
                name: Some("general".into()),
                kind: ChannelType::Text,
                tier: None,
                topic: None,
                tags: None,
                permissions: None,
                metadata: None,
            },
        )
        .await
        .expect("a channel");

    let (connection, mut events) = ws::connect(&client, WsOptions::default())
        .await
        .expect("the WebSocket sign-in");
    let subscribed = connection
        .subscribe(Subscribe {
            channels: vec![channel.id.clone()],
            include: None,
            since: None,
        })
        .await
        .expect("subscribe");
    println!("subscribed: {subscribed:?}");
    connection.ping().await.expect("ping");

    // `message.create` over the socket: the reply is the `message.created`,
    // which is a timeline change and so reaches `Events` too.
    let reply = connection
        .request(Command::MessageCreate(ChannelMessageCreate {
            group_id: group.id.clone(),
            channel_id: channel.id.clone(),
            message: message("ws"),
        }))
        .await
        .expect("message.create");
    let Event::MessageCreated(created) = reply.body else {
        panic!("message.create answered {}", reply.body.kind());
    };
    let mut posted = vec![created.message.id.clone()];
    wait_for_created(&mut events, &posted[0], &channel.id).await;

    if skipped("rest-post") {
        println!("skipped: the REST post");
    } else {
        let id = client
            .post_message(&group.id, &channel.id, &message("rest"))
            .await
            .expect("a REST post")
            .message()
            .id
            .clone();
        wait_for_created(&mut events, &id, &channel.id).await;
        posted.push(id);
    }

    let page = client
        .messages(&group.id, &channel.id, &PageRequest::newest())
        .await
        .expect("the history");
    for id in &posted {
        assert!(
            page.items
                .iter()
                .any(|item| matches!(item, TimelineItem::Message(m) if &m.id == id)),
            "{id} is in the history"
        );
    }

    connection.close().await;
    client.sign_out().await.expect("sign out");
}

/// Whether `OFSCP_INTEROP_SKIP` (comma-separated) names `check`.
fn skipped(check: &str) -> bool {
    std::env::var("OFSCP_INTEROP_SKIP").is_ok_and(|skip| skip.split(',').any(|s| s.trim() == check))
}

/// Read events until the `message.created` for `id`.
async fn wait_for_created(events: &mut Events, id: &LocalId, channel: &LocalId) {
    let created = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match events.recv().await.expect("an event") {
                Incoming::Event(frame) => match frame.body {
                    Event::MessageCreated(created) if &created.message.id == id => return created,
                    other => println!("event: {}", other.kind()),
                },
                Incoming::Closed(reason) => panic!("closed: {reason}"),
            }
        }
    })
    .await
    .expect("message.created over the socket");
    assert_eq!(&created.channel_id, channel);
}

fn message(via: &str) -> MessageCreate {
    MessageCreate {
        client_message_id: Some(format!("interop-{via}-{}", std::process::id())),
        kind: None,
        content: Content {
            text: format!("hello over {via}"),
            mime: "text/plain".into(),
        },
        attachments: None,
        reference: None,
        tags: None,
        mentions: None,
        title: None,
        summary: None,
        cover: None,
        metadata: None,
    }
}
