//! Social accounts as the window and agents see them, and the profile
//! (§6.3): its name, about line and picture.

use postrinch_store::{
    AvatarChange, DraftMedia, ProfileChange, SocialAccount, SocialAccountState, SocialChange,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::media::is_picture;
use super::overlay::SocialPending;
use super::{new_id, now, str_at};
use crate::{CommandError, CommandResult, Commands};

/// A social account, with its profile as the person last changed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialAccountInfo {
    /// Its actor, `handle@domain`.
    pub actor: String,
    pub state: SocialAccountState,
    /// The profile's `displayName`; `None` when it has none.
    pub display_name: Option<String>,
    /// The profile's about line (`bio`).
    pub bio: Option<String>,
    pub avatar: SocialAvatar,
    /// The last change to the profile still on its way, if any.
    pub pending: Option<SocialPending>,
}

/// A profile's picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "avatar")]
pub enum SocialAvatar {
    None,
    /// At its provider's address.
    Url {
        url: String,
    },
    /// Chosen here, on its way to the provider.
    Pending {
        media: DraftMedia,
    },
}

impl Commands {
    /// The social accounts this caller sees, by key.
    pub fn social_accounts(&self) -> CommandResult<Vec<SocialAccountInfo>> {
        let mut out = Vec::new();
        for account in self.store.social_accounts()? {
            if self.sees_social(&account.actor) {
                out.push(self.social_account_info(&account)?);
            }
        }
        Ok(out)
    }

    /// The social account of `actor`, in any spelling.
    pub fn social_account(&self, actor: &str) -> CommandResult<SocialAccountInfo> {
        let account = self.check_social(actor)?;
        self.social_account_info(&account)
    }

    /// The social account of `actor`, when this caller sees it and the
    /// store has it.
    pub(crate) fn check_social(&self, actor: &str) -> CommandResult<SocialAccount> {
        let unknown = || CommandError::UnknownSocialAccount(actor.to_owned());
        if !self.sees_social(actor) {
            return Err(unknown());
        }
        self.store.social_account(actor)?.ok_or_else(unknown)
    }

    /// Change the account's profile: what `change` names, the rest left as
    /// it is. It shows changed at once and reaches the provider through the
    /// outbox, a new picture uploaded first.
    pub fn change_social_profile(
        &self,
        actor: &str,
        change: &ProfileChange,
    ) -> CommandResult<SocialAccountInfo> {
        let account = self.check_social(actor)?;
        if change.display_name.is_none() && change.bio.is_none() && change.avatar.is_none() {
            return Err(CommandError::Invalid(
                "Nothing to change: give a name, an about line or a picture.".into(),
            ));
        }
        let mut change = change.clone();
        if let Some(name) = &mut change.display_name {
            *name = name.trim().to_owned();
            if name.is_empty() {
                return Err(CommandError::Invalid("A name can't be empty.".into()));
            }
        }
        if let Some(bio) = &mut change.bio {
            *bio = bio.trim().to_owned();
        }
        if let Some(AvatarChange::Set { media }) = &change.avatar {
            self.check_upload(&account.actor, media)?;
            if !is_picture(media) {
                return Err(CommandError::Invalid(
                    "A profile picture must be a picture (PNG, JPEG, GIF or WebP).".into(),
                ));
            }
        }
        self.store.queue_social_change(
            &account.actor,
            &new_id(),
            None,
            &SocialChange::Profile(change),
            now(),
        )?;
        self.social_account_info(&account)
    }

    /// An account as shown: its profile with the pending changes laid
    /// over it, in the order they were made.
    fn social_account_info(&self, account: &SocialAccount) -> CommandResult<SocialAccountInfo> {
        let profile = account.profile.as_ref().unwrap_or(&Value::Null);
        let mut info = SocialAccountInfo {
            actor: account.actor.clone(),
            state: account.state,
            display_name: str_at(profile, "displayName").map(str::to_owned),
            bio: str_at(profile, "bio").map(str::to_owned),
            avatar: str_at(profile, "avatar").map_or(SocialAvatar::None, |url| SocialAvatar::Url {
                url: url.to_owned(),
            }),
            pending: None,
        };
        for entry in self.store.social_outbox(&account.actor)? {
            let SocialChange::Profile(change) = &entry.change else {
                continue;
            };
            if let Some(name) = &change.display_name {
                info.display_name = Some(name.clone());
            }
            if let Some(bio) = &change.bio {
                info.bio = Some(bio.clone());
            }
            match &change.avatar {
                Some(AvatarChange::Set { media }) => {
                    info.avatar = SocialAvatar::Pending {
                        media: media.clone(),
                    }
                }
                Some(AvatarChange::Remove) => info.avatar = SocialAvatar::None,
                None => {}
            }
            info.pending = Some(SocialPending::of(&entry, None));
        }
        Ok(info)
    }
}
