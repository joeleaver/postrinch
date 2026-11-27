//! Media fetched once, kept on disk, and served from there again; the
//! cache kept to its size.

use std::time::Duration;

use postrinch_social::{MediaOptions, MediaRef, SocialError};

use super::{bytes, media_fetches, uploaded};
use crate::common::home;
use crate::live::user;

#[tokio::test]
async fn media_is_fetched_once_then_served_from_the_cache() {
    let h = home().await;
    h.add().await;
    let (bob, client) = user(&h, "bob").await;
    let picture = bytes(5, 4096);
    let attachment = uploaded(&client, "picture.png", picture.clone()).await;
    let media = MediaRef::attachment(&bob, &attachment);

    let first = h.social.fetch_media(&h.account(), &media).await.unwrap();
    assert!(!first.from_cache);
    assert_eq!(first.bytes, picture);
    assert_eq!(first.media.mime.as_deref(), Some("image/png"));
    assert_eq!(first.media.size, Some(picture.len() as u64));
    assert_eq!(media_fetches(&h.provider), 1);

    let again = h.social.fetch_media(&h.account(), &media).await.unwrap();
    assert!(again.from_cache);
    assert_eq!(again.bytes, picture);
    assert_eq!(media_fetches(&h.provider), 1);

    // Asked for at once by two, it is fetched once all the same.
    h.social
        .store()
        .evict_social_media(first.media.blob.as_deref().unwrap())
        .unwrap();
    let account = h.account();
    let (a, b) = tokio::join!(
        h.social.fetch_media(&account, &media),
        h.social.fetch_media(&account, &media)
    );
    assert_eq!(
        (a.unwrap().bytes, b.unwrap().bytes),
        (picture.clone(), picture)
    );
    assert_eq!(media_fetches(&h.provider), 2);
}

#[tokio::test]
async fn the_cache_evicts_what_was_used_longest_ago() {
    let h = home().await;
    h.add().await;
    let social = h.social.clone().with_media_options(MediaOptions {
        cache_bytes: 2500,
        ..MediaOptions::default()
    });
    let (bob, client) = user(&h, "bob").await;
    let mut media = Vec::new();
    for seed in 0..3 {
        let name = format!("p{seed}.png");
        let attachment = uploaded(&client, &name, bytes(seed, 1000)).await;
        media.push(MediaRef::attachment(&bob, &attachment));
    }
    let fetch = |m: &MediaRef| {
        let (social, account, m) = (social.clone(), h.account(), m.clone());
        async move { social.fetch_media(&account, &m).await.unwrap() }
    };

    fetch(&media[0]).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    fetch(&media[1]).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    // The third goes over the cache's size: the first, used longest ago,
    // goes.
    fetch(&media[2]).await;
    assert_eq!(media_fetches(&h.provider), 3);
    assert!(social.store().social_media_cache_size().unwrap() <= 2500);

    assert!(fetch(&media[1]).await.from_cache);
    assert!(fetch(&media[2]).await.from_cache);
    assert_eq!(media_fetches(&h.provider), 3);
    let first = fetch(&media[0]).await;
    assert!(!first.from_cache);
    assert_eq!(media_fetches(&h.provider), 4);
    assert!(social.store().social_media_cache_size().unwrap() <= 2500);
}

#[tokio::test]
async fn media_larger_than_the_cache_or_unlike_its_hash_is_not_kept() {
    let h = home().await;
    h.add().await;
    let (bob, client) = user(&h, "bob").await;
    let attachment = uploaded(&client, "picture.png", bytes(9, 4096)).await;

    let small = h.social.clone().with_media_options(MediaOptions {
        cache_bytes: 1024,
        ..MediaOptions::default()
    });
    let media = MediaRef::attachment(&bob, &attachment);
    let too_large = small.fetch_media(&h.account(), &media).await.unwrap_err();
    assert!(matches!(
        too_large,
        SocialError::MediaTooLarge {
            size: 4096,
            limit: 1024,
            ..
        }
    ));
    assert_eq!(media_fetches(&h.provider), 0);

    let mut damaged = media.clone();
    damaged.hash = Some(format!("sha-256:{}", "A".repeat(43) + "="));
    let unlike = h
        .social
        .fetch_media(&h.account(), &damaged)
        .await
        .unwrap_err();
    assert!(matches!(unlike, SocialError::MediaDamaged { .. }));
    assert_eq!(h.social.store().social_media_cache_size().unwrap(), 0);
}
