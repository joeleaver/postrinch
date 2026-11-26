//! Notifications from each social account's home provider.

use super::{KEY, RECORD, SocialNotification, scoped, social_account_key};
use crate::codec::FieldsBuilder;
use crate::{Store, StoreResult};

const NOTIFICATION: &str = "SocialNotification";

impl Store {
    /// Keep a notification, replacing the one with its id. One read here
    /// stays read, whatever the provider last said.
    pub fn put_social_notification(&self, notification: &SocialNotification) -> StoreResult<()> {
        let key = scoped(&notification.account, &notification.id);
        self.social_locked(|| {
            let kept: Option<SocialNotification> = self.social_record(NOTIFICATION, KEY, &key)?;
            let notification = &SocialNotification {
                read: notification.read || kept.is_some_and(|kept| kept.read),
                ..notification.clone()
            };
            self.write_social_notification(&key, notification)
        })
    }

    fn write_social_notification(
        &self,
        key: &str,
        notification: &SocialNotification,
    ) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .str("account", social_account_key(&notification.account))
            .datetime("created", notification.created)
            .bool("read", notification.read)
            .json(RECORD, notification);
        self.social_put(NOTIFICATION, KEY, key, fields)?;
        Ok(())
    }

    /// Up to `limit` of an account's notifications, newest first.
    pub fn social_notifications(
        &self,
        account: &str,
        limit: usize,
    ) -> StoreResult<Vec<SocialNotification>> {
        let mut found: Vec<SocialNotification> =
            self.social_records_where(NOTIFICATION, "account", &social_account_key(account))?;
        found.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.id.cmp(&b.id)));
        found.truncate(limit);
        Ok(found)
    }

    /// How many of an account's notifications are unread.
    pub fn social_unread_notifications(&self, account: &str) -> StoreResult<usize> {
        let mut unread = 0;
        for found in self.social_where(NOTIFICATION, "account", &social_account_key(account))? {
            if !found.bool("read")? {
                unread += 1;
            }
        }
        Ok(unread)
    }

    /// Mark an account's notifications `ids` read. Returns how many were
    /// here and unread.
    pub fn set_social_notifications_read(&self, account: &str, ids: &[&str]) -> StoreResult<usize> {
        let _held = self.social_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut marked = 0;
        for id in ids {
            let Some(found) = self.find(NOTIFICATION, KEY, &scoped(account, id))? else {
                continue;
            };
            let mut notification: SocialNotification = found.json(RECORD)?;
            if notification.read {
                continue;
            }
            notification.read = true;
            let fields = FieldsBuilder::new()
                .bool("read", true)
                .json(RECORD, &notification)
                .build();
            self.db_update(NOTIFICATION, found.id(), fields)?;
            marked += 1;
        }
        Ok(marked)
    }

    /// Forget every notification of `account`.
    pub(super) fn remove_social_notifications(&self, account: &str) -> StoreResult<usize> {
        self.social_remove_where(NOTIFICATION, "account", &social_account_key(account))
    }
}
