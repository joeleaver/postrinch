use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};

use super::*;

#[test]
fn retries_rate_limits_not_permissions() {
    assert!(is_rate_limit(StatusCode::TOO_MANY_REQUESTS, &[]));
    assert!(is_server_error(StatusCode::SERVICE_UNAVAILABLE));
    assert!(is_rate_limit(
        StatusCode::FORBIDDEN,
        &["userRateLimitExceeded".into()]
    ));
    assert!(!is_rate_limit(
        StatusCode::FORBIDDEN,
        &["insufficientPermissions".into()]
    ));
    assert!(!is_rate_limit(StatusCode::NOT_FOUND, &[]));
    assert!(!is_server_error(StatusCode::NOT_FOUND));
}

#[test]
fn backoff_grows_and_caps() {
    assert!(backoff(1, None) >= Duration::from_secs(1));
    assert!(backoff(1, None) < Duration::from_millis(1300));
    assert!(backoff(20, None) <= MAX_BACKOFF + MAX_BACKOFF / 4);
    assert_eq!(
        backoff(1, Some(Duration::from_secs(3))),
        Duration::from_secs(3)
    );
}

#[test]
fn decodes_raw_padded_or_not() {
    let mut m = RawMessage {
        id: "m".into(),
        thread_id: "t".into(),
        label_ids: vec![],
        snippet: String::new(),
        internal_date: "0".into(),
        raw: URL_SAFE_NO_PAD.encode(b"Subject: hi\r\n\r\n?>"),
    };
    assert_eq!(m.bytes().unwrap(), b"Subject: hi\r\n\r\n?>");
    m.raw = URL_SAFE.encode(b"Subject: hi\r\n\r\n?>");
    assert_eq!(m.bytes().unwrap(), b"Subject: hi\r\n\r\n?>");
}

#[test]
fn uploads_go_to_gmail_s_upload_address() {
    assert_eq!(
        upload_base("https://gmail.googleapis.com/gmail/v1/users/me"),
        "https://gmail.googleapis.com/upload/gmail/v1/users/me"
    );
    assert_eq!(
        upload_base("http://127.0.0.1:9/x"),
        "http://127.0.0.1:9/x/upload"
    );
    let body = related(
        &serde_json::json!({"threadId": "t"}),
        b"Subject: hi\r\n\r\nbody",
    );
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.starts_with("--postrinch-upload-7f3a9c1e5b\r\n"));
    assert!(text.contains("{\"threadId\":\"t\"}"));
    assert!(text.contains("message/rfc822\r\n\r\nSubject: hi"));
    assert!(text.ends_with("--postrinch-upload-7f3a9c1e5b--\r\n"));
}
