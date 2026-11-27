//! Avatars (§6.3): the picture a user's profile names, cached as other
//! media is.
//!
//! An avatar has no attachment id, so it is kept as the user's
//! (`@handle`) at their home provider. Its URL is stable while the profile
//! uses it (§6.3), so the cached picture is served until it is older than
//! [`super::MediaOptions::avatar_ttl`]; then the profile is read again, and
//! the picture fetched again only if the profile names another.

use postrinch_ofscp::ofscp_core::Actor;

use super::fetch::{CachedMedia, MediaRef, fetch_key};
use super::now_ms;
use crate::{Social, SocialResult, parse_actor};

impl Social {
    /// `user`'s avatar, as `actor`'s account sees it; `None` when their
    /// profile has none. Served from the cache while it is fresh enough,
    /// else read from the profile at `user`'s home provider (a signed
    /// request, which their privacy settings may refuse).
    pub async fn fetch_avatar(&self, actor: &str, user: &str) -> SocialResult<Option<CachedMedia>> {
        let me = parse_actor(actor)?;
        let user = parse_actor(user)?;
        let account = self.account(&me).await?;
        let wanted = avatar_ref(&user, String::new());
        let _fetching = self
            .media
            .fetching
            .hold(fetch_key(&account.actor, &wanted))
            .await;
        let (name, issuer, id) = (
            account.actor.clone(),
            wanted.issuer.clone(),
            wanted.id.clone(),
        );
        let kept = self
            .blocking(move |store| store.social_media(&name, &issuer, &id))
            .await?;
        let ttl = i64::try_from(self.media.options.avatar_ttl.as_millis()).unwrap_or(i64::MAX);
        if let Some(kept) = &kept
            && kept
                .fetched
                .is_some_and(|at| now_ms().saturating_sub(at) < ttl)
            && let Some(hit) = self
                .cached(&account.actor, &avatar_ref(&user, kept.url.clone()))
                .await?
        {
            return Ok(Some(hit));
        }

        let identity = self.identity(&account).await?;
        let client = self.client(&identity, user.domain());
        let profile = client.profile(&user).await;
        let profile = self.checked(&identity, &client, profile).await?;
        let Some(url) = profile.avatar else {
            if kept.is_some() {
                // The picture goes from the cache when nothing else names it.
                let (name, issuer, id) = (account.actor.clone(), wanted.issuer, wanted.id);
                self.blocking(move |store| store.remove_social_media(&name, &issuer, &id))
                    .await?;
            }
            return Ok(None);
        };
        let wanted = avatar_ref(&user, url);
        // The same picture, still cached: good for another while.
        if kept.is_some_and(|k| k.url == wanted.url)
            && let Some(mut hit) = self.cached(&account.actor, &wanted).await?
        {
            hit.media.fetched = Some(now_ms());
            if self.keep_if_cached(&hit.media).await? {
                return Ok(Some(hit));
            }
        }
        self.fetch_into_cache(&account.actor, &wanted)
            .await
            .map(Some)
    }
}

/// `user`'s avatar at `url`, as media to fetch.
fn avatar_ref(user: &Actor, url: String) -> MediaRef {
    MediaRef {
        issuer: user.domain().to_string(),
        id: format!("@{}", user.handle()),
        url,
        mime: None,
        size: None,
        hash: None,
        width: None,
        height: None,
        duration_ms: None,
        poster: None,
    }
}
