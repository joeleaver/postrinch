//! The groups a social account belongs to or has seen.

use super::{KEY, RECORD, SocialGroup, scoped, social_account_key};
use crate::codec::FieldsBuilder;
use crate::{Store, StoreResult};

const GROUP: &str = "SocialGroup";

impl Store {
    /// Keep a group, replacing what its account kept for its URI.
    pub fn put_social_group(&self, group: &SocialGroup) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .str("account", social_account_key(&group.account))
            .str("provider", group.provider.to_ascii_lowercase())
            .json(RECORD, group);
        self.social_put(GROUP, KEY, &scoped(&group.account, &group.uri), fields)?;
        Ok(())
    }

    /// The group at `uri`, as `account` knows it.
    pub fn social_group(&self, account: &str, uri: &str) -> StoreResult<Option<SocialGroup>> {
        self.social_record(GROUP, KEY, &scoped(account, uri))
    }

    /// The groups `account` knows, by URI.
    pub fn social_groups(&self, account: &str) -> StoreResult<Vec<SocialGroup>> {
        let mut groups: Vec<SocialGroup> =
            self.social_records_where(GROUP, "account", &social_account_key(account))?;
        groups.sort_by(|a, b| a.uri.cmp(&b.uri));
        Ok(groups)
    }

    /// The groups of every account held by the provider at `domain`.
    pub fn social_groups_on(&self, domain: &str) -> StoreResult<Vec<SocialGroup>> {
        let mut groups: Vec<SocialGroup> =
            self.social_records_where(GROUP, "provider", &domain.to_ascii_lowercase())?;
        groups.sort_by(|a, b| (&a.account, &a.uri).cmp(&(&b.account, &b.uri)));
        Ok(groups)
    }

    /// Forget a group, with its channels and their messages. Views keep
    /// naming it, so it shows again if the account joins it again.
    pub fn remove_social_group(&self, account: &str, uri: &str) -> StoreResult<()> {
        for place in self.social_places_of_group(account, uri)? {
            self.remove_social_place(account, &place.id)?;
        }
        self.social_remove(GROUP, KEY, &scoped(account, uri))?;
        Ok(())
    }
}
