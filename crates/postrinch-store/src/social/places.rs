//! Channels and private conversations, as each social account knows them.

use rhypedb_engine::CompareOp;

use super::{KEY, RECORD, SocialPlace, scoped, social_account_key};
use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreResult};

pub(super) const PLACE: &str = "SocialPlace";

impl Store {
    /// Keep a place, replacing what its account kept for its id.
    pub fn put_social_place(&self, place: &SocialPlace) -> StoreResult<()> {
        self.social_locked(|| self.write_social_place(place))
    }

    /// Keep `fresh`, merged by `keep` with the place already here, if
    /// any, with no other change to a social object in between: a read
    /// marker moved or a conversation muted while sync fetched `fresh`
    /// is not written over. `keep` must not call the store: the social
    /// lock is held while it runs.
    pub fn keep_social_place(
        &self,
        fresh: SocialPlace,
        keep: impl FnOnce(SocialPlace, SocialPlace) -> SocialPlace,
    ) -> StoreResult<SocialPlace> {
        self.social_locked(|| {
            let place = match self.social_place(&fresh.account, &fresh.id)? {
                Some(kept) => keep(kept, fresh),
                None => fresh,
            };
            self.write_social_place(&place)?;
            Ok(place)
        })
    }

    fn write_social_place(&self, place: &SocialPlace) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .str("account", social_account_key(&place.account))
            .opt_str("group", place.group.clone())
            .str("kind", place.kind.name())
            .bool("followed", place.followed)
            .opt_datetime("last_activity", place.last_activity)
            .json(RECORD, place);
        self.social_put(PLACE, KEY, &scoped(&place.account, &place.id), fields)?;
        Ok(())
    }

    /// The place `id` (a channel URI or a dmId), as `account` knows it.
    pub fn social_place(&self, account: &str, id: &str) -> StoreResult<Option<SocialPlace>> {
        self.social_record(PLACE, KEY, &scoped(account, id))
    }

    /// Change the place as it is now, with no other change to a social
    /// object in between: sync moving its cursors and the window moving
    /// its read marker lose nothing to each other. `change` must not call
    /// the store: the social lock is held while it runs. Returns it as changed,
    /// or `None` when it is not here.
    pub fn change_social_place(
        &self,
        account: &str,
        id: &str,
        change: impl FnOnce(&mut SocialPlace),
    ) -> StoreResult<Option<SocialPlace>> {
        self.social_locked(|| {
            let Some(mut place) = self.social_place(account, id)? else {
                return Ok(None);
            };
            change(&mut place);
            self.write_social_place(&place)?;
            Ok(Some(place))
        })
    }

    /// The places `account` knows, by id.
    pub fn social_places(&self, account: &str) -> StoreResult<Vec<SocialPlace>> {
        let places = self.social_records_where(PLACE, "account", &social_account_key(account))?;
        Ok(by_id(places))
    }

    /// The channels of the group at `uri` that `account` knows, by id.
    pub fn social_places_of_group(
        &self,
        account: &str,
        uri: &str,
    ) -> StoreResult<Vec<SocialPlace>> {
        let account = social_account_key(account);
        let places = self
            .social_records_where::<SocialPlace>(PLACE, "group", uri)?
            .into_iter()
            .filter(|place| social_account_key(&place.account) == account)
            .collect();
        Ok(by_id(places))
    }

    /// The places every account follows, the most recently active first
    /// (those with no message yet last).
    pub fn social_followed_places(&self) -> StoreResult<Vec<SocialPlace>> {
        let found = self
            .db
            .filter_scan_bool(PLACE, "followed", CompareOp::Eq, true, None)?;
        let mut places = found
            .into_iter()
            .map(|o| Fields::new(PLACE, o).json(RECORD))
            .collect::<StoreResult<Vec<SocialPlace>>>()?;
        places.sort_by(|a, b| {
            b.last_activity
                .cmp(&a.last_activity)
                .then_with(|| (&a.account, &a.id).cmp(&(&b.account, &b.id)))
        });
        Ok(places)
    }

    /// Forget a place, with its messages and the changes to it still on
    /// their way out. Drafts written for it stay, with no place, for the
    /// person to send elsewhere or delete.
    pub fn remove_social_place(&self, account: &str, id: &str) -> StoreResult<()> {
        let key = scoped(account, id);
        self.remove_social_outbox_of_place(&key)?;
        self.unplace_social_drafts(&key)?;
        self.remove_social_messages_of_place(&key)?;
        self.social_remove(PLACE, KEY, &key)?;
        Ok(())
    }
}

/// Sorted by account, then id.
fn by_id(mut places: Vec<SocialPlace>) -> Vec<SocialPlace> {
    places.sort_by(|a, b| (&a.account, &a.id).cmp(&(&b.account, &b.id)));
    places
}
