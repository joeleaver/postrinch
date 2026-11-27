//! Media upload (§5.8): progress, a cut-off body, the size limit.

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
