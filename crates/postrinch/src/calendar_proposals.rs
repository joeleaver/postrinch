//! Agents' calendar changes that wait for the person (DESIGN.md
//! "Calendar"): each shows in the sidebar's "needs you" list; a made or
//! changed event opens in the editor, with the agent's reason over it; a
//! deletion or an answer opens the event, with the agent's suggestion on
//! it. Making the change settles it; Dismiss drops it.

use jiff::tz::TimeZone;
use postrinch_commands::Commands;
use postrinch_model::{EmailAddress, EventChange, EventProposal};

use crate::actions::commands;
use crate::calendar_edit::Form;
use crate::calendar_layout::span_day;
use crate::state::{State, View};

impl State {
    /// Show a proposal where the person can make it.
    pub fn open_event_proposal(self, account: &EmailAddress, p: &EventProposal) {
        let tz = TimeZone::system();
        self.view.set(View::Calendar);
        if let Some(day) = span_day(&p.change.when(), &tz) {
            self.calendar_go(day, None);
        }
        match &p.change {
            EventChange::Save { event, draft } => {
                let mut form = Form::of_draft(account.clone(), event.clone(), draft.clone(), &tz);
                form.proposal = Some(p.clone());
                self.calendar.open.set(None);
                self.calendar.editing.set(Some(form));
            }
            EventChange::Delete { event, .. } | EventChange::Answer { event, .. } => {
                self.calendar.editing.set(None);
                self.calendar
                    .pending
                    .set(Some((account.clone(), event.clone())));
            }
        }
    }

    pub fn dismiss_event_proposal(self, account: &EmailAddress, id: u64) {
        if let Some(here) = commands()
            && let Err(e) = here.settle_event_proposal(account, id)
        {
            tracing::warn!("dropping an agent's event: {e}");
        }
        self.calendar.editing.update(|e| {
            if let Some(f) = e
                && f.proposal.as_ref().is_some_and(|p| p.id == id)
            {
                f.proposal = None;
            }
        });
        self.revision.update(|n| *n += 1);
    }

    /// The agent's deletion or answer waiting on an event.
    pub fn proposal_for(self, account: &EmailAddress, event: &str) -> Option<EventProposal> {
        self.calendar.proposals.with(|all| {
            all.iter()
                .find(|(a, p)| {
                    a == account
                        && matches!(
                            p.change,
                            EventChange::Delete { .. } | EventChange::Answer { .. }
                        )
                        && p.change.event() == Some(event)
                })
                .map(|(_, p)| p.clone())
        })
    }
}

/// The person changed `event` themselves: what agents asked of it goes.
pub fn settle_for(here: &Commands, account: &EmailAddress, event: &str) {
    let Ok(all) = here.event_proposals() else {
        return;
    };
    for (a, p) in all {
        if &a == account
            && p.change.event() == Some(event)
            && let Err(e) = here.settle_event_proposal(account, p.id)
        {
            tracing::warn!("settling an agent's event: {e}");
        }
    }
}
