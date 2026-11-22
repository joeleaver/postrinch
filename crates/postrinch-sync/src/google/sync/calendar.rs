//! Invitations in mail, looked up and answered in the account's Google
//! Calendar. Gmail puts an invitation into the invited account's calendar
//! as it arrives, so the event is found there by its `UID`, and answering
//! it there tells the organizer as Google Calendar does.

use postrinch_model::{Answer, CalendarEvent, EmailAddress, Invitation};

use super::GmailSync;
use crate::SyncResult;
use crate::google::api::{EventResource, is_time, response_status, to_event};

/// What answering an invitation came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answered {
    /// Answered: the event as the calendar now has it.
    Done(Box<CalendarEvent>),
    /// The account's calendar does not have the event.
    NotInCalendar,
    /// The event is there, but the account is not among its guests (it
    /// organizes it, or was invited through a group).
    NotAGuest,
}

impl GmailSync {
    /// The event an invitation describes, as the account's calendar has it;
    /// `None` when the calendar does not have it.
    pub async fn invitation_event(
        &self,
        account: &EmailAddress,
        invitation: &Invitation,
    ) -> SyncResult<Option<CalendarEvent>> {
        Ok(self
            .event_for(account, invitation)
            .await?
            .as_ref()
            .and_then(to_event))
    }

    /// Answer an invitation as the account: in its calendar, telling the
    /// organizer.
    pub async fn answer_invitation(
        &self,
        account: &EmailAddress,
        invitation: &Invitation,
        answer: Answer,
    ) -> SyncResult<Answered> {
        let Some(event) = self.event_for(account, invitation).await? else {
            return Ok(Answered::NotInCalendar);
        };
        let mut guests = event.attendees.clone();
        let Some(me) = guests.iter_mut().find(|a| a.me) else {
            return Ok(Answered::NotAGuest);
        };
        me.response_status = Some(response_status(answer).to_owned());
        let changed = self.api.set_attendees(account, &event.id, &guests).await?;
        Ok(match to_event(&changed) {
            Some(event) => Answered::Done(Box::new(event)),
            None => Answered::NotInCalendar,
        })
    }

    /// The calendar's copy of the event: the one time of a repeating event
    /// the invitation names, else the event itself.
    async fn event_for(
        &self,
        account: &EmailAddress,
        invitation: &Invitation,
    ) -> SyncResult<Option<EventResource>> {
        let events = self.api.events_by_uid(account, &invitation.uid).await?;
        Ok(match &invitation.recurrence {
            Some(time) => events.into_iter().find(|e| is_time(e, time)),
            None => events.into_iter().find(|e| e.original_start_time.is_none()),
        })
    }
}
