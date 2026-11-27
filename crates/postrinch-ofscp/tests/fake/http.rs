//! Signed requests: sign-in, the 503 retry, the clock skew, problems.

use std::collections::HashSet;
use std::time::Duration;

use ofscp_core::wire::ErrorCode;
use postrinch_ofscp::Error;
use postrinch_ofscp_fake::{Authentication, FakeProvider};

use crate::common::{requests_to, signed_in, transport, world};

#[tokio::test]
async fn signing_in_registers_a_key_that_signs_requests() {
    let provider = FakeProvider::start().await.unwrap();
    let alice = signed_in(&transport(), &provider, "alice").await;
    let me = alice.me().await.expect("GET /api/me");
    assert_eq!(me.profile.handle, "alice");
    assert_eq!(me.profile.domain, provider.authority());
    let keys = alice.device_keys().await.expect("the device keys");
    assert!(
        keys.keys
            .iter()
            .any(|k| k.key_id == alice.identity().key_id())
    );
    let [request] = &requests_to(&provider, "/api/me")[..] else {
        panic!("one GET /api/me");
    };
    assert_eq!(
        request.authentication,
        Authentication::Verified {
            actor: alice.identity().actor().clone(),
            key_id: alice.identity().key_id().to_owned(),
        }
    );
}

#[tokio::test]
async fn a_503_is_retried_and_each_retry_is_signed_afresh() {
    let w = world().await;
    w.provider.faults().fail_next(2, None);
    w.alice.me().await.expect("the third attempt answers");
    let tries = requests_to(&w.provider, "/api/me");
    let statuses: Vec<u16> = tries.iter().map(|r| r.status).collect();
    assert_eq!(statuses, [503, 503, 200]);
    let nonces: HashSet<_> = tries.iter().map(|r| r.nonce.clone().unwrap()).collect();
    assert_eq!(nonces.len(), 3, "a fresh nonce each time (§4.5)");
}

#[tokio::test]
async fn too_many_503s_come_back_as_the_problem() {
    let w = world().await;
    // No Retry-After, so the backoff is the policy's own (20, 40, 80 ms).
    w.provider.faults().fail_next(10, None);
    let Err(Error::Problem(problem)) = w.alice.me().await else {
        panic!("the 503 comes back");
    };
    assert_eq!(problem.status, 503);
    assert_eq!(problem.error_code, Some(ErrorCode::ServiceUnavailable));
    // The first attempt and three retries.
    assert_eq!(requests_to(&w.provider, "/api/me").len(), 4);
}

#[tokio::test]
async fn a_long_retry_after_is_left_to_the_caller() {
    let w = world().await;
    w.provider
        .faults()
        .fail_next(1, Some(Duration::from_secs(3600)));
    let Err(Error::Problem(problem)) = w.alice.me().await else {
        panic!("the 503 comes back");
    };
    assert_eq!(problem.retry_after, Some(Duration::from_secs(3600)));
    assert_eq!(requests_to(&w.provider, "/api/me").len(), 1);
}

#[tokio::test]
async fn a_skewed_provider_clock_is_learned_and_the_request_signed_again() {
    let w = world().await;
    w.provider.faults().set_clock_offset(400);
    w.alice
        .me()
        .await
        .expect("signed again with the provider's time");
    let statuses: Vec<u16> = requests_to(&w.provider, "/api/me")
        .iter()
        .map(|r| r.status)
        .collect();
    assert_eq!(statuses, [401, 200]);
    let offset = w.alice.provider().clock().offset_secs();
    assert!((395..=405).contains(&offset), "offset {offset}");
    // Now known, the next request is right the first time.
    w.alice.me().await.unwrap();
    assert_eq!(requests_to(&w.provider, "/api/me").len(), 3);
}

#[tokio::test]
async fn a_revoked_key_is_refused_and_not_signed_again() {
    let w = world().await;
    w.alice
        .revoke_device_key(w.alice.identity().key_id())
        .await
        .expect("the key is revoked");
    let Err(Error::Problem(problem)) = w.alice.me().await else {
        panic!("a revoked key is refused");
    };
    assert_eq!(problem.status, 401);
    assert_eq!(requests_to(&w.provider, "/api/me").len(), 1);
}

#[tokio::test]
async fn a_wrong_password_is_a_problem() {
    let provider = FakeProvider::start().await.unwrap();
    provider.add_user("alice", "the-right-password-123");
    let Err(Error::Problem(problem)) = transport()
        .provider(postrinch_ofscp::Endpoint::new(provider.authority()))
        .login("alice", "the-wrong-password-123")
        .await
    else {
        panic!("refused");
    };
    assert_eq!(problem.status, 401);
}
