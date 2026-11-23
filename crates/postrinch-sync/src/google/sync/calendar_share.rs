//! Sharing the person's calendars, and adding or removing calendars others
//! shared with them. Google tells whoever a calendar is shared with.

use postrinch_model::{CalendarInfo, CalendarRole, CalendarShare, EmailAddress, ShareWho};

use super::GmailSync;
use crate::SyncResult;
use crate::google::api::to_calendar;

impl GmailSync {
    /// Who one of the account's own calendars is shared with, and how.
    pub async fn calendar_shares(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> SyncResult<Vec<CalendarShare>> {
        self.api.calendar_acl(account, calendar).await
    }

    /// Share a calendar with someone, or change what they may do; Google
    /// emails them.
    pub async fn share_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
        who: &ShareWho,
        role: CalendarRole,
    ) -> SyncResult<CalendarShare> {
        self.api
            .share_calendar(account, calendar, who, role, true)
            .await
    }

    /// Stop sharing a calendar by one of its rules.
    pub async fn unshare_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
        rule: &str,
    ) -> SyncResult<()> {
        self.api.unshare_calendar(account, calendar, rule).await
    }

    /// Add a calendar shared with the account to its calendars, and bring
    /// its events here.
    pub async fn add_shared_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> SyncResult<CalendarInfo> {
        let listed = self.api.add_calendar(account, calendar).await?;
        let info = to_calendar(&listed);
        let (who, kept) = (account.clone(), info.clone());
        self.with_store(move |s| s.put_calendar(&who, &kept))
            .await?;
        if let Err(e) = self.sync_calendar_now(account, &info.id).await {
            tracing::warn!(
                "{account}: syncing calendar {} after adding it: {e}",
                info.id
            );
        }
        Ok(info)
    }

    /// Take a calendar off the account's calendars, and its events from
    /// here. Someone else's calendar is not deleted; they may share again.
    pub async fn remove_shared_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> SyncResult<()> {
        self.api.remove_listed_calendar(account, calendar).await?;
        let (who, cal) = (account.clone(), calendar.to_owned());
        self.with_store(move |s| s.remove_calendar(&who, &cal))
            .await
    }
}
