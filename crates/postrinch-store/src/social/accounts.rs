//! Social accounts and providers, and removing an account with everything
//! it holds.

use super::{KEY, RECORD, SocialAccount, SocialAccountState, SocialProvider, social_account_key};
use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreResult};

pub(super) const ACCOUNT: &str = "SocialAccount";
const PROVIDER: &str = "SocialProvider";
/// The unique field of a provider.
const DOMAIN: &str = "domain";

impl Store {
    /// Keep a social account, replacing what was kept for its actor.
    pub fn put_social_account(&self, account: &SocialAccount) -> StoreResult<()> {
        self.social_locked(|| self.write_social_account(account))
    }

    fn write_social_account(&self, account: &SocialAccount) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .str("state", account.state.name())
            .json(RECORD, account);
        self.social_put(ACCOUNT, KEY, &social_account_key(&account.actor), fields)?;
        Ok(())
    }

    /// The social account of `actor`, in any spelling.
    pub fn social_account(&self, actor: &str) -> StoreResult<Option<SocialAccount>> {
        self.social_record(ACCOUNT, KEY, &social_account_key(actor))
    }

    /// Every social account, by key.
    pub fn social_accounts(&self) -> StoreResult<Vec<SocialAccount>> {
        let mut accounts = Vec::new();
        for object in self.db.scan_type_projected(ACCOUNT, &[KEY, RECORD])? {
            let fields = Fields::new(ACCOUNT, object);
            accounts.push((fields.str(KEY)?, fields.json(RECORD)?));
        }
        accounts.sort_by(|a: &(String, SocialAccount), b| a.0.cmp(&b.0));
        Ok(accounts.into_iter().map(|(_, account)| account).collect())
    }

    /// The social accounts in `state`, by key.
    pub fn social_accounts_in(&self, state: SocialAccountState) -> StoreResult<Vec<SocialAccount>> {
        let mut accounts: Vec<SocialAccount> =
            self.social_records_where(ACCOUNT, "state", state.name())?;
        accounts.sort_by_key(|a| social_account_key(&a.actor));
        Ok(accounts)
    }

    /// Change the social account of `actor` as it is now, with no other
    /// change to a social object in between. `change` must not call the
    /// store: the social lock is held while it runs. Returns it as changed, or
    /// `None` when it is not here.
    pub fn change_social_account(
        &self,
        actor: &str,
        change: impl FnOnce(&mut SocialAccount),
    ) -> StoreResult<Option<SocialAccount>> {
        self.social_locked(|| {
            let Some(mut account) = self.social_account(actor)? else {
                return Ok(None);
            };
            change(&mut account);
            self.write_social_account(&account)?;
            Ok(Some(account))
        })
    }

    /// Remove a social account and everything it holds: its outbox, drafts,
    /// notifications, media, messages, places and groups, and its places
    /// and groups from every view. Providers stay: other accounts may use
    /// them. Fetched media bytes stay in the blob store until
    /// [`Store::collect_garbage`]. Removing an account that is not here
    /// does nothing, and an interrupted removal is finished by a repeat.
    /// Stop the account's sync first: what it writes meanwhile is left
    /// until a repeat.
    pub fn remove_social_account(&self, actor: &str) -> StoreResult<()> {
        let account = social_account_key(actor);
        // Its changes first, so nothing is sent for an account half gone.
        self.remove_social_outbox(&account)?;
        self.remove_social_drafts(&account)?;
        self.remove_social_notifications(&account)?;
        self.remove_social_media_of(&account)?;
        for place in self.social_places(&account)? {
            self.remove_social_place(&account, &place.id)?;
        }
        for group in self.social_groups(&account)? {
            self.remove_social_group(&account, &group.uri)?;
        }
        // Messages of places that were never kept, too.
        self.remove_social_messages_of(&account)?;
        self.forget_social_account_in_views(&account)?;
        self.social_remove(ACCOUNT, KEY, &account)?;
        Ok(())
    }

    /// Keep what is known of a provider, replacing what was kept for its
    /// domain.
    pub fn put_social_provider(&self, provider: &SocialProvider) -> StoreResult<()> {
        let fields = FieldsBuilder::new().json(RECORD, provider);
        let domain = provider.domain.to_ascii_lowercase();
        self.social_put(PROVIDER, DOMAIN, &domain, fields)?;
        Ok(())
    }

    /// What is known of the provider at `domain`, in any case.
    pub fn social_provider(&self, domain: &str) -> StoreResult<Option<SocialProvider>> {
        self.social_record(PROVIDER, DOMAIN, &domain.to_ascii_lowercase())
    }
}
