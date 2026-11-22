//! Agents' changes to the person's calendars that would tell other people
//! (invite, update or cancel guests, or answer an organizer). They wait
//! here, on this computer, until the person makes them or drops them.

use jiff::tz::TimeZone;
use postrinch_model::{CalendarEvent, EmailAddress, EventChange, EventProposal};

use crate::calendar_times::{between, span};
use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// Keep an agent's calendar change for the person. Returns it, with
    /// its id.
    pub fn propose_event_change(
        &self,
        account: &EmailAddress,
        change: EventChange,
        agent: &str,
        reason: Option<String>,
    ) -> CommandResult<EventProposal> {
        self.check(account)?;
        let proposal = EventProposal {
            id: 0,
            change,
            agent: agent.to_owned(),
            reason: reason.filter(|r| !r.trim().is_empty()),
            made: crate::views::now(),
        };
        Ok(self.store.put_event_proposal(account, proposal)?)
    }

    /// Every calendar change agents proposed, account by account, oldest
    /// first. One whose event is over is dropped.
    pub fn event_proposals(&self) -> CommandResult<Vec<(EmailAddress, EventProposal)>> {
        let now = crate::views::now();
        let tz = TimeZone::system();
        let mut all = Vec::new();
        for account in self.visible_accounts()? {
            let account = account.address;
            for p in self.store.event_proposals(&account)? {
                if span(&p.change.when(), &tz).1 <= now {
                    self.store.remove_event_proposal(&account, p.id)?;
                } else {
                    all.push((account.clone(), p));
                }
            }
        }
        Ok(all)
    }

    /// The change was made, or dropped.
    pub fn settle_event_proposal(&self, account: &EmailAddress, id: u64) -> CommandResult<()> {
        self.check(account)?;
        Ok(self.store.remove_event_proposal(account, id)?)
    }

    /// One of the account's events as kept here, by the calendar it is in
    /// and its id: a single event, or one time of a repeating one (as
    /// list_events names it, `<series>_<start>`).
    pub fn event_by_id(
        &self,
        account: &EmailAddress,
        calendar: &str,
        id: &str,
    ) -> CommandResult<CalendarEvent> {
        self.check(account)?;
        let records: Vec<_> = self
            .store
            .events(account)?
            .into_iter()
            .filter(|(c, _)| c == calendar)
            .map(|(_, r)| r)
            .collect();
        if let Some(r) = records
            .iter()
            .find(|r| r.event.id == id && r.recurrence.is_empty())
        {
            return Ok(r.event.clone());
        }
        // One time of a series: worked out around the time its id names.
        if let Some(at) = id.rsplit_once('_').and_then(|(_, t)| instance_time(t)) {
            let day = 86_400_000;
            let from = postrinch_model::Timestamp::from_millis(at - 2 * day);
            let to = postrinch_model::Timestamp::from_millis(at + 2 * day);
            if let Some(found) = between(&records, from, to, &TimeZone::system())
                .into_iter()
                .find(|e| e.id == id)
            {
                return Ok(found);
            }
        }
        Err(CommandError::Invalid(format!(
            "{account} has no event {id:?} in calendar {calendar:?}: list_events names them."
        )))
    }
}

/// The moment an instance id's suffix names: `20261124T150000Z` or
/// `20261124`, in milliseconds.
fn instance_time(text: &str) -> Option<i64> {
    let at = if text.len() == 8 {
        jiff::civil::Date::strptime("%Y%m%d", text)
            .ok()?
            .to_zoned(TimeZone::UTC)
            .ok()?
            .timestamp()
    } else {
        let wall = text.strip_suffix('Z')?;
        jiff::civil::DateTime::strptime("%Y%m%dT%H%M%S", wall)
            .ok()?
            .to_zoned(TimeZone::UTC)
            .ok()?
            .timestamp()
    };
    Some(at.as_millisecond())
}
