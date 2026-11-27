//! Login, bootstrap tokens and device keys (§4.1–§4.7).

mod common;

use common::{anonymous, http, reply, user};
use ofscp_core::signing::DeviceKey;
use postrinch_ofscp_fake::{Authentication, FakeProvider};
use rand_core::OsRng;
use reqwest::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn discovery_advertises_the_upload_limit() {
    let p = FakeProvider::start().await.unwrap();
    p.set_max_upload_bytes(1234);
    let r = anonymous(&p, Method::GET, "/.well-known/ofscp", None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["capabilities"]["limits"]["maxUploadBytes"], 1234);
}

#[tokio::test]
async fn registration_is_closed() {
    let p = FakeProvider::start().await.unwrap();
    let body = json!({"handle": "zed", "password": "pw"});
    let r = anonymous(&p, Method::POST, "/api/auth/register", Some(&body)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json["errorCode"], "registration_closed");
}

async fn bootstrap(p: &FakeProvider, handle: &str, password: &str) -> common::Reply {
    let body = json!({"handle": handle, "password": password});
    anonymous(p, Method::POST, "/api/auth/login", Some(&body)).await
}

async fn register(p: &FakeProvider, token: &str, key: &DeviceKey) -> common::Reply {
    let body = json!({"public_key": key.public_key_base64(), "device_name": "laptop"});
    let request = http()
        .post(format!("{}/api/auth/device-keys", p.base_url()))
        .bearer_auth(token)
        .json(&body);
    reply(request.send().await.unwrap()).await
}

#[tokio::test]
async fn login_then_one_key_registration_per_token() {
    let p = FakeProvider::start().await.unwrap();
    let alice = p.add_user("alice", "hunter2");
    assert_eq!(alice.handle(), "alice");
    assert_eq!(
        bootstrap(&p, "alice", "wrong").await.status,
        StatusCode::UNAUTHORIZED
    );
    let login = bootstrap(&p, "alice", "hunter2").await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.json);
    assert_eq!(login.json["token_type"], "bootstrap");
    assert_eq!(login.json["expires_in"], 300);
    let token = login.json["bootstrap_token"].as_str().unwrap().to_owned();

    let key = DeviceKey::generate(&mut OsRng);
    let created = register(&p, &token, &key).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let key_id = created.json["key_id"].as_str().unwrap().to_owned();
    assert_eq!(
        p.requests().pop().unwrap().authentication,
        Authentication::Bearer
    );
    // The token is single-use.
    let again = register(&p, &token, &DeviceKey::generate(&mut OsRng)).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);

    // The keys endpoint lists the new key.
    let keys = anonymous(&p, Method::GET, "/.well-known/ofscp/users/alice/keys", None).await;
    assert_eq!(keys.status, StatusCode::OK);
    let listed = keys.json["keys"].as_array().unwrap();
    assert!(
        listed
            .iter()
            .any(|k| k["key_id"] == key_id.as_str() && k["public_key"] == key.public_key_base64())
    );
    assert!(keys.json["cache_until"].is_string());
}

#[tokio::test]
async fn an_unknown_handle_has_no_keys() {
    let p = FakeProvider::start().await.unwrap();
    let r = anonymous(
        &p,
        Method::GET,
        "/.well-known/ofscp/users/nobody/keys",
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["keys"], json!([]));
}

#[tokio::test]
async fn a_user_lists_and_revokes_its_keys() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let r = alice
        .send(&p, Method::GET, "/api/auth/device-keys", None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["keys"].as_array().unwrap().len(), 1);
    let path = format!("/api/auth/device-keys/{}", alice.key_id);
    let r = alice.send(&p, Method::DELETE, &path, None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = alice
        .send(&p, Method::GET, "/api/auth/device-keys", None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}
