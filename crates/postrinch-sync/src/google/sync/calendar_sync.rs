//! Keeping an account's calendars here: its calendar list, then each
//! calendar's events, from where the last sync of it got to. A calendar
//! that has forgotten where that was (410) is synced again from the start.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use postrinch_model::{EmailAddress, EventRecord};

use super::GmailSync;
use crate::google::api::{EventResource, to_calendar, to_record};
use crate::{SyncError, SyncResult};

/// How long a calendar that cannot sync by token waits between syncs
/// from the start.
const REST: Duration = Duration::from_secs(6 * 3600);

/// What a calendar sync did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CalendarReport {
    pub calendars: usize,
    /// Events new or changed.
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
    /// and removed. A sync from the start keeps what changed and removes
    /// what is no longer listed, rather than clearing first, so unchanged
    /// events are not written again.
    async fn sync_events(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> SyncResult<(usize, usize)> {
        let rest_key = format!("{}\u{1f}{calendar}", account.key());
        if self.resting(&rest_key) {
            return Ok((0, 0));
        }
        let (who, cal) = (account.clone(), calendar.to_owned());
        let mut token = self
            .with_store(move |s| s.calendar_sync_token(&who, &cal))
            .await?;
        let (mut kept, mut removed) = (0, 0);
        let mut page: Option<String> = None;
        // Every event a sync from the start lists, to remove the rest.
        let mut listed: HashSet<String> = HashSet::new();
        loop {
            let got = self
                .api
                .events_page(account, calendar, token.as_deref(), page.as_deref())
                .await;
            let got = match got {
                Err(SyncError::Api { status: 410, .. }) if token.is_some() => {
                    tracing::info!("{account}: calendar {calendar} syncs again from the start");
                    // Some calendars (Google's holidays) refuse even the
                    // token they just gave: those rest between full syncs.
                    self.rest(&rest_key);
                    token = None;
                    page = None;
                    listed.clear();
                    continue;
                }
                other => other?,
            };
            tracing::debug!(
                "{account}: calendar {calendar}: {} changes{}{}",
                got.items.len(),
                if token.is_some() {
                    " since the last sync"
                } else {
                    " (from the start)"
                },
                if got.next_sync_token.is_some() {
                    ", done"
                } else {
                    ""
                }
            );
            let from_start = token.is_none();
            if from_start {
                listed.extend(got.items.iter().map(|r| r.id.clone()));
            }
            let changes: Vec<Change> = got
                .items
                .iter()
                .filter_map(|r| change(r, calendar))
                .collect();
            let (who, cal) = (account.clone(), calendar.to_owned());
            let next_sync = got.next_sync_token.clone();
            let done = got.next_page_token.is_none();
            let gone = (done && from_start).then(|| listed.clone());
            let (k, r) = self
                .with_store(move |s| {
                    let (mut k, mut r) = (0, 0);
                    for change in changes {
                        match change {
                            Change::Keep(record) => {
                                if s.put_event(&who, &cal, &record)? {
                                    k += 1;
                                }
                            }
                            Change::Remove(id) => {
                                s.remove_event(&who, &cal, &id)?;
                                r += 1;
                            }
                        }
                    }
                    if let Some(listed) = gone {
                        r += s.remove_events_except(&who, &cal, &listed)?;
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

    /// Whether a calendar that refused its own token was synced from the
    /// start too lately to do it again.
    fn resting(&self, key: &str) -> bool {
        let rested = self
            .calendars_resting
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        rested.get(key).is_some_and(|at| at.elapsed() < REST)
    }

    fn rest(&self, key: &str) {
        self.calendars_resting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned(), Instant::now());
    }
}
