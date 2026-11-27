//! The signer's own account and the §6 privacy endpoints: profile,
//! presence, group listing and visibility settings.
//!
//! `/api/me/...` calls are home-only. The `/api/users/{userRef}/...` reads
//! go to the subject's home provider, which serves only users it hosts
//! (§6.1), and also take anonymous viewers.

use ofscp_core::Actor;
use ofscp_core::wire::{
    Presence, PresenceUpdate, PrivacySettings, PrivacyUpdate, ProfileUpdate, UserAccount,
    UserGroups, UserProfile,
};
use reqwest::Method;

use super::path::{Query, path};
use super::{Client, Provider};
use crate::Result;

impl Provider {
    /// A user's profile as an anonymous viewer (§6.2): the base profile,
    /// plus extras only if their `profileVisibility` is `public`.
    pub async fn profile(&self, user: &Actor) -> Result<UserProfile> {
        self.get_anonymous(user_path(user, "profile"), Query::new())
            .await
    }

    /// A user's presence as an anonymous viewer (§6.4); `offline` unless
    /// their `presenceVisibility` is `public`.
    pub async fn presence(&self, user: &Actor) -> Result<Presence> {
        self.get_anonymous(user_path(user, "presence"), Query::new())
            .await
    }

    /// The `public`/`discoverable` groups this provider hosts that a user
    /// is in, as an anonymous viewer may see them (§6.5).
    pub async fn user_groups(&self, user: &Actor) -> Result<UserGroups> {
        self.get_anonymous(user_path(user, "groups"), Query::new())
            .await
    }
}

impl Client {
    /// The signer's own account (§5.1.2), at home.
    pub async fn me(&self) -> Result<UserAccount> {
        self.require_home()?;
        self.get("/api/me".to_owned(), Query::new()).await
    }

    /// Change the signer's profile (§6.3), at home; absent fields stay. An
    /// avatar is an `Attachment` URL from [`Client::upload`].
    pub async fn update_profile(&self, update: &ProfileUpdate) -> Result<UserProfile> {
        self.require_home()?;
        self.send_json(Method::PATCH, "/api/me/profile".to_owned(), update)
            .await
    }

    /// A user's profile as the signer may see it (§6.2), at the user's
    /// home. An unknown user, or one hosted elsewhere, is a `404`.
    pub async fn profile(&self, user: &Actor) -> Result<UserProfile> {
        self.get(user_path(user, "profile"), Query::new()).await
    }

    /// A user's presence as the signer may see it (§6.4), at the user's
    /// home; a viewer not permitted sees `offline`.
    pub async fn presence(&self, user: &Actor) -> Result<Presence> {
        self.get(user_path(user, "presence"), Query::new()).await
    }

    /// Set the signer's presence (§6.4), at home. A full replace: an
    /// absent `status` clears it.
    pub async fn set_presence(&self, update: &PresenceUpdate) -> Result<Presence> {
        self.require_home()?;
        self.send_json(Method::PUT, "/api/me/presence".to_owned(), update)
            .await
    }

    /// The groups the user's home hosts that they are in and the signer
    /// may see (§6.5). Remote groups are never listed.
    pub async fn user_groups(&self, user: &Actor) -> Result<UserGroups> {
        self.get(user_path(user, "groups"), Query::new()).await
    }

    /// The signer's visibility settings (§6.6), at home.
    pub async fn privacy(&self) -> Result<PrivacySettings> {
        self.require_home()?;
        self.get("/api/me/privacy".to_owned(), Query::new()).await
    }

    /// Change the signer's visibility settings (§6.6), at home: a partial
    /// update; `[]` clears a list. Answers the full settings.
    pub async fn update_privacy(&self, update: &PrivacyUpdate) -> Result<PrivacySettings> {
        self.require_home()?;
        self.send_json(Method::PUT, "/api/me/privacy".to_owned(), update)
            .await
    }
}

/// `/api/users/{userRef}/{tail}`, the user as a canonical actor (§2.2).
fn user_path(user: &Actor, tail: &str) -> String {
    let mut path = path("/api/users/{}/", &[&user.to_string()]);
    path.push_str(tail);
    path
}
