//! Uploads (§5.8) and the upload faults.

mod common;

use std::time::{Duration, Instant};

use common::{Part, Tweak, multipart, reply, user};
use postrinch_ofscp_fake::FakeProvider;
use reqwest::{Method, StatusCode};

async fn upload(p: &FakeProvider, u: &common::User, parts: &[Part<'_>]) -> common::Reply {
    let (body, content_type) = multipart(parts);
    u.send_raw(
        p,
        Method::POST,
        "/api/media",
        body,
        Some(&content_type),
        Tweak::default(),
    )
    .await
}

#[tokio::test]
async fn an_upload_is_stored_and_served_at_its_capability_url() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let r = upload(
        &p,
        &alice,
        &[
            ("file", Some("notes.txt"), Some("text/plain"), b"hello file"),
            ("width", None, None, b"10"),
        ],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.json);
    assert_eq!(r.json["mime"], "text/plain");
    assert_eq!(r.json["size"], 10);
    assert_eq!(r.json["filename"], "notes.txt");
    assert_eq!(r.json["width"], 10);
    assert!(r.json["hash"].as_str().unwrap().starts_with("sha-256:"));
    let url = r.json["url"].as_str().unwrap();
    assert!(url.starts_with(&format!("{}/media/", p.base_url())));

    // Fetched unsigned; not shown inline.
    let served = reply(common::http().get(url).send().await.unwrap()).await;
    assert_eq!(served.status, StatusCode::OK);
    assert_eq!(served.headers["content-disposition"], "attachment");
    assert_eq!(served.headers["x-content-type-options"], "nosniff");
    assert_eq!(served.headers["cache-control"], "private");
    let bytes = common::http()
        .get(url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"hello file");
}

#[tokio::test]
async fn a_thumbnail_must_be_a_picture() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let parts: [Part<'_>; 2] = [
        ("file", Some("v.mp4"), Some("video/mp4"), b"video"),
        ("thumbnail", Some("t.txt"), Some("text/plain"), b"nope"),
    ];
    assert_eq!(
        upload(&p, &alice, &parts).await.status,
        StatusCode::BAD_REQUEST
    );
    let parts: [Part<'_>; 2] = [
        ("file", Some("v.mp4"), Some("video/mp4"), b"video"),
        ("thumbnail", Some("t.png"), Some("image/png"), b"png"),
    ];
    let r = upload(&p, &alice, &parts).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.json);
    assert_eq!(r.json["thumbnail"]["mime"], "image/png");
    assert!(r.json["thumbnail"].get("thumbnail").is_none());
}

#[tokio::test]
async fn an_upload_over_the_limit_is_413() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.set_max_upload_bytes(8);
    let r = upload(
        &p,
        &alice,
        &[(
            "file",
            Some("big.bin"),
            Some("application/octet-stream"),
            b"0123456789",
        )],
    )
    .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.json["errorCode"], "payload_too_large");
}

#[tokio::test]
async fn the_cut_fault_drops_the_connection_mid_upload() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.faults().cut_uploads_after(Some(100));
    let big = vec![b'x'; 64 * 1024];
    let (body, content_type) = multipart(&[(
        "file",
        Some("big.bin"),
        Some("application/octet-stream"),
        &big,
    )]);
    let headers = alice.sign(
        &p.authority(),
        "POST",
        "/api/media",
        &body,
        &Tweak::default(),
    );
    let mut request = alice
        .http
        .post(format!("{}/api/media", p.base_url()))
        .header("content-type", content_type)
        .body(body);
    for (k, v) in headers {
        request = request.header(k, v);
    }
    assert!(
        request.send().await.is_err(),
        "the connection is dropped, so no response arrives"
    );
    // Other requests still work.
    p.faults().cut_uploads_after(None);
    assert_eq!(
        alice.send(&p, Method::GET, "/api/me", None).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_slow_fault_throttles_uploads() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.faults().slow_uploads(Some(20 * 1024));
    let data = vec![b'y'; 10 * 1024];
    let started = Instant::now();
    let r = upload(
        &p,
        &alice,
        &[(
            "file",
            Some("s.bin"),
            Some("application/octet-stream"),
            &data,
        )],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.json);
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "took {:?}",
        started.elapsed()
    );
}
