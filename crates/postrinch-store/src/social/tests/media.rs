//! Media: what is known of attachments, the bytes picked to upload, and
//! the cache of fetched bytes, kept to a size.

use std::time::Duration;

use super::accounts::{draft, media};
use super::*;

/// `account`'s attachment `id`, cached as `bytes` at `fetched`.
fn cached(store: &Store, account: &str, id: &str, bytes: &[u8], fetched: i64) -> SocialMedia {
    let mut picture = media(account, id);
    picture.blob = Some(store.cache_social_media_bytes(bytes).unwrap());
    picture.size = Some(bytes.len() as u64);
    picture.fetched = Some(fetched);
    store.put_social_media(&picture).unwrap();
    picture
}

#[test]
fn media_is_found_by_attachment_and_url() {
    let (_dir, store) = store();
    let picture = media(ADA, "a1");
    store.put_social_media(&picture).unwrap();
    store.put_social_media(&media(BOB, "a1")).unwrap();
    assert_eq!(
        store.social_media(ADA, "Groups.Example", "a1").unwrap(),
        Some(picture.clone())
    );
    assert_eq!(store.social_media_at(&picture.url).unwrap().len(), 2);

    let picture = cached(&store, ADA, "a1", b"png bytes", 5);
    let read = store.social_media(ADA, "groups.example", "a1").unwrap();
    assert_eq!(read.unwrap().blob, picture.blob);
    assert_eq!(
        store
            .cached_social_media_bytes(picture.blob.as_deref().unwrap())
            .unwrap()
            .as_deref(),
        Some(&b"png bytes"[..])
    );
    store
        .remove_social_media(ADA, "groups.example", "a1")
        .unwrap();
    assert_eq!(
        store.social_media(ADA, "groups.example", "a1").unwrap(),
        None
    );
}

#[test]
fn kept_bytes_and_cached_bytes_live_apart() {
    let (_dir, store) = store();
    let kept = store.put_social_kept_bytes(b"pasted").unwrap();
    let cached = store.cache_social_media_bytes(b"fetched").unwrap();
    assert_eq!(store.cached_social_media_bytes(&kept).unwrap(), None);
    assert_eq!(store.social_kept_bytes(&cached).unwrap(), None);
    // The same bytes in both are two copies: evicting one keeps the other.
    let both = store.put_social_kept_bytes(b"both").unwrap();
    assert_eq!(store.cache_social_media_bytes(b"both").unwrap(), both);
    store.evict_social_media(&both).unwrap();
    assert!(store.social_kept_bytes(&both).unwrap().is_some());
    assert_eq!(store.cached_social_media_bytes(&both).unwrap(), None);
}

#[test]
fn garbage_collection_keeps_what_media_and_drafts_refer_to() {
    let (_dir, store) = store();
    let fetched = cached(&store, ADA, "a1", b"fetched", 1).blob.unwrap();
    let unreferenced = store.cache_social_media_bytes(b"no one's").unwrap();
    let pasted = store.put_social_kept_bytes(b"pasted").unwrap();
    let dropped = store.put_social_kept_bytes(b"no one's either").unwrap();
    let mut writing = draft(ADA, "d1", None);
    writing.attachments.push(DraftMedia::Kept {
        blob: pasted.clone(),
        name: "paste.png".into(),
        mime: Some("image/png".into()),
    });
    store.put_social_draft(&writing).unwrap();

    assert_eq!(store.collect_garbage(Duration::ZERO).unwrap(), 2);
    assert!(store.cached_social_media_bytes(&fetched).unwrap().is_some());
    assert!(
        store
            .cached_social_media_bytes(&unreferenced)
            .unwrap()
            .is_none()
    );
    assert!(store.social_kept_bytes(&pasted).unwrap().is_some());
    assert!(store.social_kept_bytes(&dropped).unwrap().is_none());
}

#[test]
fn the_cache_evicts_the_least_recently_used_first() {
    let (_dir, store) = store();
    let old = cached(&store, ADA, "a1", &[1; 40], 1);
    let used = cached(&store, ADA, "a2", &[2; 40], 2);
    let new = cached(&store, ADA, "a3", &[3; 40], 3);
    // The same bytes as a2, for Bob: one blob, counted once.
    let mut shared = media(BOB, "b2");
    shared.blob = used.blob.clone();
    shared.size = Some(40);
    shared.fetched = Some(2);
    store.put_social_media(&shared).unwrap();
    assert_eq!(store.social_media_cache_size().unwrap(), 120);

    // a2 is asked for after a3 was fetched: a1 is now the oldest, then a3.
    let mut asked = used.clone();
    asked.used = Some(10);
    assert!(store.put_social_media_if_cached(&asked).unwrap());
    let trimmed = store.trim_social_media(80, None).unwrap();
    assert_eq!(
        trimmed,
        Trimmed {
            evicted: 1,
            freed: 40,
            size: 80
        }
    );
    let forgotten = store.social_media(ADA, "groups.example", "a1").unwrap();
    let forgotten = forgotten.expect("the record is kept");
    assert_eq!((forgotten.blob, forgotten.fetched), (None, None));
    assert_eq!(forgotten.mime, old.mime);
    assert_eq!(
        store
            .cached_social_media_bytes(old.blob.as_deref().unwrap())
            .unwrap(),
        None
    );

    // What was just fetched is kept even when it alone is over the limit.
    let trimmed = store.trim_social_media(10, new.blob.as_deref()).unwrap();
    assert_eq!(trimmed.evicted, 1);
    assert_eq!(trimmed.size, 40);
    for (account, id) in [(ADA, "a2"), (BOB, "b2")] {
        let record = store.social_media(account, "groups.example", id).unwrap();
        assert_eq!(record.unwrap().blob, None, "{account}'s {id} is evicted");
    }
    let kept = store.social_media(ADA, "groups.example", "a3").unwrap();
    assert_eq!(kept.unwrap().blob, new.blob);
    assert_eq!(store.social_media_cache_size().unwrap(), 40);
}

#[test]
fn cached_bytes_no_record_names_go_at_once() {
    let (_dir, store) = store();
    let shared = cached(&store, ADA, "a1", b"shared", 1);
    cached(&store, BOB, "a1", b"shared", 1);
    let blob = shared.blob.clone().unwrap();

    // Ada's record names other bytes now; Bob's still names these.
    let mut changed = shared.clone();
    changed.blob = Some(store.cache_social_media_bytes(b"changed").unwrap());
    store.put_social_media(&changed).unwrap();
    assert!(store.cached_social_media_bytes(&blob).unwrap().is_some());

    // Bob's goes, and with it the last record naming them.
    store
        .remove_social_media(BOB, "groups.example", "a1")
        .unwrap();
    assert!(store.cached_social_media_bytes(&blob).unwrap().is_none());

    // An account removed takes the bytes only it named.
    let changed_blob = changed.blob.unwrap();
    store.remove_social_account(ADA).unwrap();
    assert!(
        store
            .cached_social_media_bytes(&changed_blob)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.social_media_cache_size().unwrap(), 0);
}

#[test]
fn fetched_bytes_are_kept_with_their_record_and_a_hit_loses_to_an_eviction() {
    let (_dir, store) = store();
    let first = cached(&store, ADA, "a1", &[1; 40], 1);
    let mut fetched = media(ADA, "a2");
    fetched.fetched = Some(2);
    let (kept, trimmed) = store.keep_social_media(&fetched, &[2; 40], 50).unwrap();
    // The new bytes are kept, named by their record; the old go.
    let blob = kept.blob.clone().unwrap();
    assert_eq!(
        store.cached_social_media_bytes(&blob).unwrap(),
        Some(vec![2; 40])
    );
    assert_eq!(
        store.social_media(ADA, "groups.example", "a2").unwrap(),
        Some(kept)
    );
    assert_eq!((trimmed.evicted, trimmed.size), (1, 40));

    // A hit read before that eviction does not bring back what it named.
    let mut stale = first.clone();
    stale.used = Some(3);
    assert!(!store.put_social_media_if_cached(&stale).unwrap());
    let forgotten = store
        .social_media(ADA, "groups.example", "a1")
        .unwrap()
        .unwrap();
    assert_eq!(forgotten.blob, None);
}
