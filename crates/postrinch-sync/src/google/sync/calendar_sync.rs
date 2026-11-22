//! Keeping an account's calendars here: its calendar list, then each
//! calendar's events, from where the last sync of it got to. A calendar
//! that has forgotten where that was (410) is synced again from the start.

use postrinch_model::{EmailAddress, EventRecord};

use super::GmailSync;
use crate::google::api::{EventResource, to_calendar, to_record};
use crate::{SyncError, SyncResult};

/// What a calendar sync did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CalendarReport {
    pub calendars: usize,
    /// Events kept or changed.
    pub kept: usize,
    pub removed: usize,
}

/// A change to one event.
enum Change {
    Keep(Box<EventRecord>),
    Remove(String),
}

fn change(r: &EventResource, calendar: &str) -> Option<Change> {
    let cancelled = r.status.as_deref() == Some("cancelled");
    if cancelled && r.recurring_event_id.is_none() {
        // A whole event deleted; a cancelled time of a series is kept, to
        // leave that time out.
        return Some(Change::Remove(r.id.clone()));
    }
    to_record(r, calendar).map(|record| Change::Keep(Box::new(record)))
}

impl GmailSync {
    /// Bring the account's calendars and their events up to date.
    pub async fn sync_calendars(&self, account: &EmailAddress) -> SyncResult<CalendarReport> {
        let listed: Vec<_> = self
            .api
            .calendar_list(account)
            .await?
            .iter()
            .map(to_calendar)
            .collect();
        let mut report = CalendarReport {
            calendars: listed.len(),
            ..Default::default()
        };
        let (who, wanted) = (account.clone(), listed.clone());
        self.with_store(move |s| {
            for kept in s.calendars(&who)? {
                if !wanted.iter().any(|c| c.id == kept.id) {
                    s.remove_calendar(&who, &kept.id)?;
                }
            }
            for info in &wanted {
                s.put_calendar(&who, info)?;
            }
            Ok(())
        })
        .await?;
        for info in listed {
            let (kept, removed) = self.sync_events(account, &info.id).await?;
            report.kept += kept;
            report.removed += removed;
        }
        Ok(report)
    }

    /// Bring one calendar's events up to date; returns how many were kept
    /// and removed.
    async fn sync_events(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> SyncResult<(usize, usize)> {
        let (who, cal) = (account.clone(), calendar.to_owned());
        let mut token = self
            .with_store(move |s| s.calendar_sync_token(&who, &cal))
            .await?;
        let (mut kept, mut removed) = (0, 0);
        let mut page: Option<String> = None;
        loop {
            let got = self
                .api
                .events_page(account, calendar, token.as_deref(), page.as_deref())
                .await;
            let got = match got {
                Err(SyncError::Api { status: 410, .. }) if token.is_some() => {
                    tracing::info!("{account}: calendar {calendar} syncs again from the start");
                    let (who, cal) = (account.clone(), calendar.to_owned());
                    self.with_store(move |s| {
                        s.set_calendar_sync_token(&who, &cal, None)?;
                        s.clear_events(&who, &cal)
                    })
                    .await?;
                    token = None;
                    page = None;
                    continue;
                }
                other => other?,
            };
            let changes: Vec<Change> = got
                .items
                .iter()
                .filter_map(|r| change(r, calendar))
                .collect();
            let (who, cal) = (account.clone(), calendar.to_owned());
            let next_sync = got.next_sync_token.clone();
            let done = got.next_page_token.is_none();
            let (k, r) = self
                .with_store(move |s| {
                    let (mut k, mut r) = (0, 0);
                    for change in changes {
                        match change {
                            Change::Keep(record) => {
                                s.put_event(&who, &cal, &record)?;
                                k += 1;
                            }
                            Change::Remove(id) => {
                                s.remove_event(&who, &cal, &id)?;
                                r += 1;
                            }
                        }
                    }
                    // Written only once the last page is kept, so a sync
                    // cut short starts that page run again.
                    if done && let Some(next) = &next_sync {
                        s.set_calendar_sync_token(&who, &cal, Some(next))?;
                    }
                    Ok((k, r))
                })
                .await?;
            kept += k;
            removed += r;
            match got.next_page_token {
                Some(next) => page = Some(next),
                None => return Ok((kept, removed)),
            }
        }
    }
}
