//! A queued change's files, uploaded before it.

use std::time::Duration;

use postrinch_social::{LiveEvent, MediaOptions, UploadProgress};
use postrinch_store::{DraftMedia, SocialChange, TakeBack};

use super::{attached, bytes, file, uploads, with_files};
use crate::common::{channel_uri, home};
use crate::live::Told;
use crate::outbox::{chat, kept_sent, outbox, queue, send};

/// The files `change` carries, as queued.
fn files(change: &SocialChange) -> &[DraftMedia] {
    match change {
        SocialChange::Post(post) => &post.content.attachments,
        other => panic!("not a post: {other:?}"),
    }
}

/// Every upload progress told so far.
fn progress(told: &Told) -> Vec<UploadProgress> {
    told.seen
        .iter()
        .filter_map(|e| match e {
            LiveEvent::Uploading(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_upload_cut_off_part_way_is_tried_again_and_the_post_then_goes() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;
    let dir = tempfile::tempdir().unwrap();
    let picture = bytes(7, 512 * 1024);
    let path = file(dir.path(), "picture.png", &picture);

    let mut told = Told::new(&h.social);
    h.provider.faults().cut_uploads_after(Some(128 * 1024));
    queue(
        &h,
        "c1",
        &place,
        &with_files("a picture", vec![attached(&path, "image/png")]),
    );
    let first = send(&h).await;
    assert_eq!((first.sent, first.failed, first.refused), (0, 1, 0));
    told.until(
        "the post not sent",
        |e| matches!(e, LiveEvent::NotSent { client_id, refused: false, .. } if client_id == "c1"),
    )
    .await;
    // Part of it went, as far as the connection lasted.
    let cut = progress(&told);
    assert!(
        cut.iter().any(|p| p.sent > 0 && p.sent < p.total),
        "told {cut:#?}"
    );
    assert!(
        cut.iter()
            .all(|p| p.client_id == "c1" && p.name == "picture.png")
    );
    assert!(cut.iter().all(|p| (p.file, p.files) == (0, 1)));
    assert!(h.social.uploads(&h.account()).unwrap().is_empty());
    // Not sent, so not an attempt: it may still be taken back.
    let waiting = outbox(&h);
    assert_eq!(waiting.len(), 1);
    assert!(!waiting[0].tried && waiting[0].attempts == 0 && waiting[0].error.is_some());
    assert!(matches!(
        files(&waiting[0].change),
        [DraftMedia::File { .. }]
    ));
    assert_eq!(uploads(&h.provider), 0);
    assert!(h.provider.messages(&group, &channel).is_empty());

    h.provider.faults().cut_uploads_after(None);
    let second = send(&h).await;
    assert_eq!((second.sent, second.failed), (1, 0));
    assert!(outbox(&h).is_empty());
    assert_eq!(uploads(&h.provider), 1);
    told.until(
        "the whole upload",
        |e| matches!(e, LiveEvent::Uploading(p) if p.total > 0 && p.sent == p.total),
    )
    .await;
    let at_provider = h.provider.messages(&group, &channel);
    assert_eq!(at_provider.len(), 1);
    let attachments = &at_provider[0].attachments;
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].size, picture.len() as u64);
    assert_eq!(attachments[0].mime, "image/png");
    let kept = kept_sent(&h, &place, "c1");
    assert_eq!(
        kept.message["attachments"][0]["id"],
        attachments[0].id.as_str()
    );
}

#[tokio::test]
async fn a_post_is_held_until_its_files_are_up_and_what_follows_waits_for_it() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "notes.txt", b"a file on disk");
    let blob = h
        .social
        .store()
        .put_social_kept_bytes(b"a pasted picture")
        .unwrap();
    let pasted = DraftMedia::Kept {
        blob,
        name: "pasted.png".into(),
        mime: Some("image/png".into()),
    };

    h.provider.faults().cut_uploads_after(Some(1));
    queue(
        &h,
        "c1",
        &place,
        &with_files("two files", vec![pasted, attached(&path, "text/plain")]),
    );
    queue(&h, "c2", &place, &chat("after it", None));
    let held = send(&h).await;
    assert_eq!((held.sent, held.failed), (0, 1));
    let waiting = outbox(&h);
    assert_eq!(waiting.len(), 2);
    assert!(!waiting[1].tried, "what follows it is not tried");
    assert!(h.provider.messages(&group, &channel).is_empty());

    h.provider.faults().cut_uploads_after(None);
    let sent = send(&h).await;
    assert_eq!((sent.sent, sent.failed), (2, 0));
    assert_eq!(uploads(&h.provider), 2);
    let at_provider = h.provider.messages(&group, &channel);
    let order: Vec<_> = at_provider
        .iter()
        .map(|m| m.client_message_id.as_deref())
        .collect();
    assert_eq!(order, [Some("c1"), Some("c2")]);
    let names: Vec<_> = at_provider[0]
        .attachments
        .iter()
        .map(|a| a.filename.as_deref())
        .collect();
    assert_eq!(names, [Some("pasted.png"), Some("notes.txt")]);
    // The uploads came before the post that carries them.
    let requests = h.provider.requests();
    let first_post = requests
        .iter()
        .position(|r| r.method == "POST" && r.path.ends_with("/messages"))
        .unwrap();
    let last_upload = requests
        .iter()
        .rposition(|r| r.method == "POST" && r.path == "/api/media")
        .unwrap();
    assert!(last_upload < first_post);
}

#[tokio::test]
async fn a_file_up_is_kept_with_the_change_and_not_uploaded_again() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;
    let dir = tempfile::tempdir().unwrap();
    let small = file(dir.path(), "small.png", &bytes(1, 1024));
    let large = file(dir.path(), "large.png", &bytes(2, 256 * 1024));

    // The first file goes up whole; the second is cut off.
    h.provider.faults().cut_uploads_after(Some(64 * 1024));
    queue(
        &h,
        "c1",
        &place,
        &with_files(
            "two pictures",
            vec![attached(&small, "image/png"), attached(&large, "image/png")],
        ),
    );
    assert_eq!(send(&h).await.failed, 1);
    let waiting = outbox(&h);
    // The change keeps its files as picked; the first's upload is kept
    // with it.
    assert!(matches!(
        files(&waiting[0].change),
        [DraftMedia::File { .. }, DraftMedia::File { .. }]
    ));
    assert_eq!(waiting[0].uploads.len(), 1);
    assert_eq!(waiting[0].uploads[0].media, attached(&small, "image/png"));
    assert_eq!(uploads(&h.provider), 1);

    // Gone from the disk now, the first is not needed again.
    std::fs::remove_file(&small).unwrap();
    h.provider.faults().cut_uploads_after(None);
    assert_eq!(send(&h).await.sent, 1);
    assert_eq!(uploads(&h.provider), 2);
    assert_eq!(
        h.provider.messages(&group, &channel)[0].attachments.len(),
        2
    );
}

#[tokio::test]
async fn a_file_up_too_long_ago_is_uploaded_again() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;
    // Nothing up is trusted to be kept: as if every retry came a day on.
    let social = h.social.clone().with_media_options(MediaOptions {
        upload_fresh: Duration::ZERO,
        ..MediaOptions::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let small = file(dir.path(), "small.png", &bytes(1, 1024));
    let large = file(dir.path(), "large.png", &bytes(2, 256 * 1024));

    h.provider.faults().cut_uploads_after(Some(64 * 1024));
    queue(
        &h,
        "c1",
        &place,
        &with_files(
            "two pictures",
            vec![attached(&small, "image/png"), attached(&large, "image/png")],
        ),
    );
    assert_eq!(social.send_outbox(&h.account()).await.unwrap().failed, 1);
    assert_eq!(uploads(&h.provider), 1);
    let first = outbox(&h)[0].uploads[0].attachment["id"].clone();
    // Times are whole milliseconds: let one go by, so the upload is older
    // than the next pass's now.
    tokio::time::sleep(Duration::from_millis(5)).await;

    h.provider.faults().cut_uploads_after(None);
    assert_eq!(social.send_outbox(&h.account()).await.unwrap().sent, 1);
    // Both went up this time, and the message carries the new upload.
    assert_eq!(uploads(&h.provider), 3);
    let sent = &h.provider.messages(&group, &channel)[0].attachments;
    assert_eq!(sent.len(), 2);
    assert_ne!(serde_json::json!(sent[0].id.as_str()), first);
}

#[tokio::test]
async fn a_change_whose_upload_failed_can_still_be_taken_back() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "picture.png", &bytes(3, 64 * 1024));

    h.provider.faults().cut_uploads_after(Some(1024));
    queue(
        &h,
        "c1",
        &place,
        &with_files("a picture", vec![attached(&path, "image/png")]),
    );
    assert_eq!(send(&h).await.failed, 1);
    let taken = h
        .social
        .store()
        .take_back_social_change(&h.account(), "c1")
        .unwrap();
    assert_eq!(taken, TakeBack::Taken);
    h.provider.faults().cut_uploads_after(None);
    assert_eq!(send(&h).await, postrinch_social::Sent::default());
    assert!(h.provider.messages(&group, &channel).is_empty());
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_refused_and_holds_back_nothing() {
    let h = home().await;
    let (_bob, group, channel) = h.bob_with_group();
    let place = channel_uri(&h.provider, &group, &channel);
    h.add().await;
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("gone.png");

    queue(
        &h,
        "c1",
        &place,
        &with_files("a picture", vec![attached(&gone, "image/png")]),
    );
    queue(&h, "c2", &place, &chat("after it", None));
    let sent = send(&h).await;
    assert_eq!((sent.sent, sent.refused), (1, 1));
    let kept = outbox(&h);
    assert_eq!(kept.len(), 1);
    assert!(kept[0].refused && !kept[0].tried);
    let at_provider = h.provider.messages(&group, &channel);
    assert_eq!(at_provider.len(), 1);
    assert_eq!(at_provider[0].client_message_id.as_deref(), Some("c2"));
}
