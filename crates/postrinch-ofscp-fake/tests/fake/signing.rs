//! §4.5 verification of REST requests, and the faults around it.

use std::time::Duration;

use crate::common::{Tweak, anonymous, user};
use postrinch_ofscp_fake::{Authentication, FakeProvider};
use reqwest::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn a_signed_request_is_verified_and_recorded() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(
        r.json["profile"]["id"],
        format!("{}/api/users/alice", p.base_url())
    );
    assert!(r.headers.contains_key("date"));
    let last = p.requests().pop().unwrap();
    assert_eq!(last.path, "/api/me");
    assert_eq!(last.status, 200);
    assert_eq!(
        last.authentication,
        Authentication::Verified {
            actor: alice.actor.clone(),
            key_id: alice.key_id.clone(),
        }
    );
}

#[tokio::test]
async fn a_query_is_part_of_what_is_signed() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let r = alice
        .send(
            &p,
            Method::GET,
            "/api/me/notifications?limit=5&type=mention",
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
}

#[tokio::test]
async fn a_reused_nonce_is_refused() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let tweak = Tweak {
        nonce: Some("n0nce-n0nce-n0nce-n0nce-1".to_owned()),
        ..Tweak::default()
    };
    let first = alice
        .send_with(&p, Method::GET, "/api/me", None, tweak.clone())
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let again = alice
        .send_with(&p, Method::GET, "/api/me", None, tweak)
        .await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    assert_eq!(again.json["errorCode"], "unauthorized");
    let last = p.requests().pop().unwrap();
    assert!(matches!(
        last.authentication,
        Authentication::Rejected { status: 401, .. }
    ));
}

#[tokio::test]
async fn clock_skew_beyond_300_seconds_is_refused() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.faults().set_clock_offset(301);
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    p.faults().set_clock_offset(250);
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::OK);
    // The Date header follows the provider's clock, so a client can see
    // the skew.
    let date = httpdate::parse_http_date(r.headers["date"].to_str().unwrap()).unwrap();
    let ahead = date.duration_since(std::time::SystemTime::now()).unwrap();
    assert!(ahead > Duration::from_secs(240));
}

#[tokio::test]
async fn a_body_that_does_not_match_its_digest_is_refused() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let tweak = Tweak {
        signed_body: Some(b"{}".to_vec()),
        ..Tweak::default()
    };
    let body = json!({"displayName": "Alice"});
    let r = alice
        .send_with(&p, Method::PATCH, "/api/me/profile", Some(&body), tweak)
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_signature_for_another_provider_is_refused() {
    let a = FakeProvider::start().await.unwrap();
    let b = FakeProvider::start().await.unwrap();
    let alice = user(&a, "alice");
    // Signed for b's authority, sent to a.
    let headers = alice.sign(&b.authority(), "GET", "/api/me", b"", &Tweak::default());
    let mut request = alice.http.get(format!("{}/api/me", a.base_url()));
    for (k, v) in headers {
        request = request.header(k, v);
    }
    let r = crate::common::reply(request.send().await.unwrap()).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_revoked_key_no_longer_verifies() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    assert!(p.faults().revoke_key(&alice.key_id));
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn fail_next_answers_signed_requests_only_with_503() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.faults().fail_next(2, Some(Duration::from_secs(7)));
    // Unsigned requests do not count.
    let r = anonymous(&p, Method::GET, "/.well-known/ofscp-provider", None).await;
    assert_eq!(r.status, StatusCode::OK);
    for _ in 0..2 {
        let r = alice.send(&p, Method::GET, "/api/me", None).await;
        assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(r.headers["retry-after"], "7");
        assert_eq!(r.json["errorCode"], "service_unavailable");
    }
    assert_eq!(
        p.requests().pop().unwrap().authentication,
        Authentication::Faulted { status: 503 }
    );
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn refuse_signatures_answers_signed_requests_only_with_401() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.faults().refuse_signatures(true);
    let r = anonymous(&p, Method::GET, "/.well-known/ofscp-provider", None).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.json["errorCode"], "unauthorized");
    assert_eq!(
        p.requests().pop().unwrap().authentication,
        Authentication::Faulted { status: 401 }
    );
    p.faults().refuse_signatures(false);
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn refusing_signatures_on_some_paths_answers_the_rest() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.faults()
        .refuse_signatures_ending("/notification-settings");
    let r = alice
        .send(&p, Method::GET, "/api/me/notification-settings", None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::OK);
    p.faults().refuse_signatures(false);
    let r = alice
        .send(&p, Method::GET, "/api/me/notification-settings", None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn a_restarted_replay_store_answers_503_while_it_warms_up() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.faults().restart_replay_store();
    assert!(p.faults().warm_up_until().is_some());
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(r.headers.contains_key("retry-after"));
}

#[tokio::test]
async fn ending_the_warm_up_serves_requests_again() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    p.faults().restart_replay_store();
    p.faults().end_warm_up();
    assert!(p.faults().warm_up_until().is_none());
    let r = alice.send(&p, Method::GET, "/api/me", None).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn an_anonymous_viewer_reads_public_groups_only() {
    let p = FakeProvider::start().await.unwrap();
    let alice = user(&p, "alice");
    let open = p.create_group(&alice.actor, "Open");
    p.configure_group(&open, |g| g.tier = ofscp_core::wire::Tier::Public);
    let closed = p.create_group(&alice.actor, "Closed");
    let r = anonymous(
        &p,
        Method::GET,
        &format!("/api/groups/{}", open.as_str()),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json.get("membership").is_none());
    let r = anonymous(
        &p,
        Method::GET,
        &format!("/api/groups/{}", closed.as_str()),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = alice
        .send(
            &p,
            Method::GET,
            &format!("/api/groups/{}", closed.as_str()),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json["membership"]["role"], "owner");
}
