//! Avatars: read from the user's profile, and cached as other media is.

use std::time::Duration;

use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::wire::{Patch, ProfileUpdate};
use postrinch_social::MediaOptions;

use super::{bytes, media_fetches, uploaded};
use crate::common::home;
use crate::live::user;

/// How many times a profile was read.
fn profile_reads(h: &crate::common::Home) -> usize {
    h.provider
        .requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.ends_with("/profile"))
        .count()
}

/// Set the avatar of whoever `client` signs as.
async fn set_avatar(client: &Client, avatar: Patch<String>) {
    let update = ProfileUpdate {
        avatar,
        ..ProfileUpdate::default()
    };
    client
        .update_profile(&update)
        .await
        .expect("profile updated");
}

#[tokio::test]
async fn an_avatar_is_fetched_from_the_profile_and_cached() {
    let h = home().await;
    h.add().await;
    let (cleo, client) = user(&h, "cleo").await;
    let cleo = cleo.to_string();
    assert!(
        h.social
            .fetch_avatar(&h.account(), &cleo)
            .await
            .unwrap()
            .is_none()
    );

    let picture = bytes(4, 2048);
    let first = uploaded(&client, "me.png", picture.clone()).await;
    set_avatar(&client, Patch::Set(first.url.clone())).await;
    let fetched = h
        .social
        .fetch_avatar(&h.account(), &cleo)
        .await
        .unwrap()
        .unwrap();
    assert!(!fetched.from_cache);
    assert_eq!(fetched.bytes, picture);
    assert_eq!(fetched.media.url, first.url);
    let (reads, fetches) = (profile_reads(&h), media_fetches(&h.provider));

    // Fresh enough: neither the profile nor the picture is read again.
    let again = h
        .social
        .fetch_avatar(&h.account(), &cleo)
        .await
        .unwrap()
        .unwrap();
    assert!(again.from_cache);
    assert_eq!(again.bytes, picture);
    assert_eq!(
        (profile_reads(&h), media_fetches(&h.provider)),
        (reads, fetches)
    );

    // Once it is old, the profile is read again; the same picture is not.
    let stale = h.social.clone().with_media_options(MediaOptions {
        avatar_ttl: Duration::ZERO,
        ..MediaOptions::default()
    });
    let checked = stale
        .fetch_avatar(&h.account(), &cleo)
        .await
        .unwrap()
        .unwrap();
    assert!(checked.from_cache);
    assert_eq!(
        (profile_reads(&h), media_fetches(&h.provider)),
        (reads + 1, fetches)
    );

    // A new picture is fetched.
    let other = bytes(8, 1024);
    let second = uploaded(&client, "me-too.png", other.clone()).await;
    set_avatar(&client, Patch::Set(second.url.clone())).await;
    let changed = stale
        .fetch_avatar(&h.account(), &cleo)
        .await
        .unwrap()
        .unwrap();
    assert!(!changed.from_cache);
    assert_eq!(changed.bytes, other);
    assert_eq!(media_fetches(&h.provider), fetches + 1);
    // The old picture, named by nothing now, is gone from the cache.
    let old = fetched.media.blob.as_deref().unwrap();
    let store = h.social.store();
    assert!(store.cached_social_media_bytes(old).unwrap().is_none());

    // None at all: nothing is served, and nothing kept for it.
    set_avatar(&client, Patch::Clear).await;
    assert!(
        stale
            .fetch_avatar(&h.account(), &cleo)
            .await
            .unwrap()
            .is_none()
    );
    let kept = store
        .social_media(&h.account(), h.provider.authority().as_str(), "@cleo")
        .unwrap();
    assert!(kept.is_none());
    let new = changed.media.blob.as_deref().unwrap();
    assert!(store.cached_social_media_bytes(new).unwrap().is_none());
}
