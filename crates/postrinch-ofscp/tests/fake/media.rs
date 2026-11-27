//! Media (§5.8): uploads with progress, a cut-off body, the size limit,
//! and fetching an upload back by its URL.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use postrinch_ofscp::Error;
use postrinch_ofscp::http::{MediaUpload, Progress, ProgressFn, Source};

use crate::common::world;

fn picture(len: usize) -> MediaUpload {
    let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    MediaUpload::new(Source::Bytes(Bytes::from(bytes)), "a.png", "image/png")
}

fn recorder() -> (ProgressFn, Arc<Mutex<Vec<Progress>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    (Arc::new(move |p| log.lock().unwrap().push(p)), seen)
}

#[tokio::test]
async fn an_upload_reports_its_progress_to_the_end() {
    let w = world().await;
    let (progress, seen) = recorder();
    let attachment = w
        .alice
        .upload(&picture(300_000), Some(progress))
        .await
        .expect("an upload");
    assert_eq!(attachment.size, 300_000);
    assert_eq!(attachment.mime, "image/png");
    let seen = seen.lock().unwrap();
    let last = seen.last().expect("progress");
    assert_eq!(last.sent, last.total);
    assert!(last.total > 300_000, "the whole form, not just the file");
    assert!(seen.windows(2).all(|p| p[0].sent <= p[1].sent));
}

#[tokio::test]
async fn a_cut_off_upload_fails() {
    let w = world().await;
    w.provider.faults().cut_uploads_after(Some(10_000));
    let result = w.alice.upload(&picture(300_000), None).await;
    assert!(matches!(result, Err(Error::Network { .. })), "{result:?}");
}

#[tokio::test]
async fn an_upload_over_the_limit_is_refused() {
    let w = world().await;
    w.provider.set_max_upload_bytes(1_000);
    let Err(Error::Problem(problem)) = w.alice.upload(&picture(5_000), None).await else {
        panic!("too large");
    };
    assert_eq!(problem.status, 413);
}

#[tokio::test]
async fn an_upload_is_fetched_back_unsigned_by_its_url() {
    let w = world().await;
    let upload = picture(20_000);
    let attachment = w.alice.upload(&upload, None).await.expect("an upload");
    let transport = crate::common::transport();
    let fetched = transport
        .fetch_media(&attachment.url, 20_000)
        .await
        .expect("a fetch");
    let expected: Vec<u8> = (0..20_000).map(|i| (i % 251) as u8).collect();
    assert_eq!(fetched.bytes, expected);
    assert_eq!(fetched.mime.as_deref(), Some("image/png"));
    let served = w
        .provider
        .requests()
        .into_iter()
        .find(|r| r.method == "GET" && r.path.starts_with("/media/"))
        .expect("the fetch reached the provider");
    assert_eq!(served.header("signature"), None);

    // Larger than asked for, gone, or not https: each refused.
    let too_large = transport.fetch_media(&attachment.url, 19_999).await;
    assert!(
        matches!(too_large, Err(Error::TooLarge { .. })),
        "{too_large:?}"
    );
    let gone = format!("{}/media/0000/a.png", w.provider.base_url());
    let Err(Error::Problem(problem)) = transport.fetch_media(&gone, 100).await else {
        panic!("no such file");
    };
    assert_eq!(problem.status, 404);
    let plain = attachment.url.replacen("https:", "http:", 1);
    let refused = transport.fetch_media(&plain, 100).await;
    assert!(
        matches!(refused, Err(Error::MediaUrl { .. })),
        "{refused:?}"
    );
}
