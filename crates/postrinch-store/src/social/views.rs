//! The social views the person made: selections of places and groups
//! across accounts, in the person's order.

use super::{KEY, RECORD, SocialView, social_account_key};
use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreResult};

const VIEW: &str = "SocialView";

impl Store {
    /// Keep a view, replacing the one with its id.
    pub fn put_social_view(&self, view: &SocialView) -> StoreResult<()> {
        self.social_locked(|| self.write_social_view(view))
    }

    fn write_social_view(&self, view: &SocialView) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .u64("order", view.order)
            .json(RECORD, view);
        self.social_put(VIEW, KEY, &view.id, fields)?;
        Ok(())
    }

    /// The view with this id.
    pub fn social_view(&self, id: &str) -> StoreResult<Option<SocialView>> {
        self.social_record(VIEW, KEY, id)
    }

    /// Every view, in the person's order (by id where two share a place).
    pub fn social_views(&self) -> StoreResult<Vec<SocialView>> {
        let mut views = self
            .db
            .scan_type_projected(VIEW, &[RECORD])?
            .into_iter()
            .map(|o| Fields::new(VIEW, o).json(RECORD))
            .collect::<StoreResult<Vec<SocialView>>>()?;
        views.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
        Ok(views)
    }

    /// Delete a view. Returns whether it was here.
    pub fn remove_social_view(&self, id: &str) -> StoreResult<bool> {
        self.social_locked(|| self.social_remove(VIEW, KEY, id))
    }

    /// Take an account's places and groups out of every view (the account
    /// is being removed). Views left empty stay: the person made them.
    pub(super) fn forget_social_account_in_views(&self, account: &str) -> StoreResult<()> {
        let account = social_account_key(account);
        self.social_locked(|| {
            for mut view in self.social_views()? {
                let (places, groups) = (view.places.len(), view.groups.len());
                view.places
                    .retain(|p| social_account_key(&p.account) != account);
                view.groups
                    .retain(|g| social_account_key(&g.account) != account);
                if (places, groups) != (view.places.len(), view.groups.len()) {
                    self.write_social_view(&view)?;
                }
            }
            Ok(())
        })
    }
}
