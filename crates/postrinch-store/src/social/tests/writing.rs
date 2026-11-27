//! Drafts, notifications and the outbox.

use std::time::Duration;

use serde_json::json;

use super::accounts::{draft, notification};
use super::*;

#[test]
fn drafts_survive_a_restart_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let older = draft(ADA, "d1", Some(GENERAL));
    let mut newer = draft(BOB, "d2", None);
    newer.updated = 20;
    newer.kind = ContentKind::Memo;
    newer.cover = Some(DraftMedia::Uploaded {
        attachment: json!({ "id": "a1", "url": "https://cdn.example/a1" }),
    });
    store.put_social_draft(&older).unwrap();
    store.put_social_draft(&newer).unwrap();
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.social_drafts().unwrap(), [newer, older.clone()]);
    assert_eq!(store.social_drafts_for(ADA, GENERAL).unwrap(), [older]);
    assert!(store.remove_social_draft("d1").unwrap());
    assert!(!store.remove_social_draft("d1").unwrap());
    assert_eq!(store.social_draft("d1").unwrap(), None);
}

#[test]
fn notifications_list_newest_first_and_are_read() {
    let (_dir, store) = store();
    for (id, created) in [("n1", 1), ("n2", 3), ("n3", 2)] {
        store
            .put_social_notification(&notification(ADA, id, created))
            .unwrap();
    }
    store
        .put_social_notification(&notification(BOB, "n9", 9))
        .unwrap();
    let ids = |found: Vec<SocialNotification>| -> Vec<String> {
        found.into_iter().map(|n| n.id).collect()
    };
    assert_eq!(
        ids(store.social_notifications(ADA, 10).unwrap()),
        ["n2", "n3", "n1"]
    );
    assert_eq!(ids(store.social_notifications(ADA, 1).unwrap()), ["n2"]);
    assert_eq!(store.social_unread_notifications(ADA).unwrap(), 3);
    assert_eq!(
        store
            .set_social_notifications_read(ADA, &["n1", "n2", "n0"])
            .unwrap(),
        2
    );
    assert_eq!(
        store.set_social_notifications_read(ADA, &["n1"]).unwrap(),
        0
    );
    assert_eq!(store.social_unread_notifications(ADA).unwrap(), 1);
    let read = store.social_notifications(ADA, 10).unwrap();
    assert!(read.iter().find(|n| n.id == "n1").unwrap().read);
    assert_eq!(store.social_unread_notifications(BOB).unwrap(), 1);
}

#[test]
fn the_outbox_keeps_the_order_changes_were_made_in() {
    let (_dir, store) = store();
    let change = |n: u32| read_change(&format!("m{n}"));
    let first = store
        .queue_social_change(ADA, "c1", Some(GENERAL), &change(1), 1)
        .unwrap();
    let second = store
        .queue_social_change(BOB, "c2", None, &change(2), 2)
        .unwrap();
    let third = store
        .queue_social_change("ADA@groups.example", "c3", Some(GENERAL), &change(3), 3)
        .unwrap();
    assert!(first.seq < second.seq && second.seq < third.seq);
    // A repeat queues nothing twice.
    let again = store
        .queue_social_change(ADA, "c1", Some(GENERAL), &change(9), 9)
        .unwrap();
    assert_eq!(again, first);

    let client_ids = |entries: Vec<SocialOutboxEntry>| -> Vec<String> {
        entries.into_iter().map(|e| e.client_id).collect()
    };
    assert_eq!(client_ids(store.social_outbox(ADA).unwrap()), ["c1", "c3"]);
    assert_eq!(
        client_ids(store.social_outbox_for(ADA, GENERAL).unwrap()),
        ["c1", "c3"]
    );
    assert_eq!(client_ids(store.social_outbox(BOB).unwrap()), ["c2"]);

    let failed = store
        .social_change_failed(ADA, "c1", "503 service unavailable")
        .unwrap()
        .unwrap();
    assert_eq!(
        (failed.attempts, failed.error.as_deref()),
        (1, Some("503 service unavailable"))
    );
    assert_eq!(store.social_outbox(ADA).unwrap()[0], failed);
    assert_eq!(store.social_change_failed(ADA, "c0", "x").unwrap(), None);

    assert!(store.remove_social_change(ADA, "c1").unwrap());
    assert!(!store.remove_social_change(ADA, "c1").unwrap());
    // The next change still comes after every one made, even once the
    // outbox has emptied, and across a restart.
    assert!(store.remove_social_change(BOB, "c2").unwrap());
    assert!(store.remove_social_change(ADA, "c3").unwrap());
    let fourth = store
        .queue_social_change(BOB, "c4", None, &change(4), 4)
        .unwrap();
    assert!(fourth.seq > third.seq);
}

#[test]
fn sequence_numbers_are_never_taken_twice_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let first = store
        .queue_social_change(ADA, "c1", None, &read_change("m1"), 1)
        .unwrap();
    store.remove_social_change(ADA, "c1").unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let second = store
        .queue_social_change(ADA, "c2", None, &read_change("m2"), 2)
        .unwrap();
    assert!(second.seq > first.seq);
}

#[test]
fn garbage_collection_keeps_what_queued_changes_upload() {
    let (_dir, store) = store();
    let pasted = store.put_social_kept_bytes(b"pasted, then sent").unwrap();
    store
        .queue_social_change(
            ADA,
            "c1",
            Some(GENERAL),
            &SocialChange::Post(SocialPost {
                kind: ContentKind::Chat,
                reply_to: None,
                content: SocialContent {
                    body: "a picture".into(),
                    attachments: vec![DraftMedia::Kept {
                        blob: pasted.clone(),
                        name: "paste.png".into(),
                        mime: Some("image/png".into()),
                    }],
                    ..SocialContent::default()
                },
            }),
            1,
        )
        .unwrap();
    assert_eq!(store.collect_garbage(Duration::ZERO).unwrap(), 0);
    assert!(store.social_kept_bytes(&pasted).unwrap().is_some());
    store.remove_social_change(ADA, "c1").unwrap();
    assert_eq!(store.collect_garbage(Duration::ZERO).unwrap(), 1);
}

#[test]
fn a_notification_read_here_stays_read() {
    let (_dir, store) = store();
    store
        .put_social_notification(&notification(ADA, "n1", 1))
        .unwrap();
    store.set_social_notifications_read(ADA, &["n1"]).unwrap();
    // The provider gives it again, from before it heard of the read.
    store
        .put_social_notification(&notification(ADA, "n1", 1))
        .unwrap();
    assert_eq!(store.social_unread_notifications(ADA).unwrap(), 0);
    assert!(store.social_notifications(ADA, 1).unwrap()[0].read);
}

#[test]
fn a_change_reads_back_as_queued_with_the_blobs_it_uploads() {
    let (_dir, store) = store();
    let avatar = store.put_social_kept_bytes(b"cropped").unwrap();
    let change = SocialChange::Profile(ProfileChange {
        display_name: Some("Ada L.".into()),
        bio: None,
        avatar: Some(AvatarChange::Set {
            media: DraftMedia::Kept {
                blob: avatar.clone(),
                name: "avatar.png".into(),
                mime: Some("image/png".into()),
            },
        }),
    });
    assert_eq!(change.blobs(), std::slice::from_ref(&avatar));
    store
        .queue_social_change(ADA, "c1", None, &change, 1)
        .unwrap();
    assert_eq!(store.social_outbox(ADA).unwrap()[0].change, change);
    assert_eq!(store.collect_garbage(Duration::ZERO).unwrap(), 0);
    let read = SocialChange::Read {
        message: "m1".into(),
    };
    assert!(read.blobs().is_empty());
}
