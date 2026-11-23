//! Invitations in the reader (DESIGN.md "Calendar"): the loader reads the
//! open thread's invitations from its messages; each one that asks for an
//! answer is then looked up in its account's Google Calendar, and answered
//! there when the person presses Yes, Maybe or No.

use std::collections::HashMap;

use postrinch_commands::{Commands, Thread, may_invite};
use postrinch_model::{
    Answer, CalendarEvent, EmailAddress, Invitation, InvitationKind, ProposedAnswer,
    ProviderMessageId,
};
use postrinch_sync::google::Answered;
use rinch::prelude::*;
use rinch::{Effect, untracked};

use crate::actions::{commands, sync};
use crate::state::State;
use crate::sync::SyncHandle;

/// Said when sync, and so the calendar, is off.
pub(crate) const OFF: &str = "Postrinch can\u{2019}t reach Google Calendar now (sync is off), so it can\u{2019}t answer this.";

/// The reader's invitations. `Copy`, like the window's state.
#[derive(Clone, Copy)]
pub struct Invitations {
    /// The open thread's invitations, by message id.
    pub found: Signal<HashMap<String, Invitation>>,
    /// What each one's account's calendar says of it, by message id.
    pub events: Signal<HashMap<String, Lookup>>,
    /// What clashes with each invitation's time, by message id.
    pub clashes: Signal<HashMap<String, Vec<CalendarEvent>>>,
    /// Agents' answers waiting for the person, in every account.
    pub proposals: Signal<Vec<(EmailAddress, ProposedAnswer)>>,
    /// The calendars the open thread's messages share, by message id.
    pub shares: Signal<HashMap<String, crate::calendar_shares::Shared>>,
}

/// An invitation's event, as far as its calendar has been asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Looking,
    Found(CalendarEvent),
    /// The person's answer, on its way to the calendar.
    Answering(CalendarEvent, Answer),
    NotInCalendar,
    /// Not in the calendar, so answered by mail to the organizer.
    Mailed(Answer),
    /// The calendar could not be asked, and why, in a sentence.
    Failed(String),
}

impl Invitations {
    pub fn new() -> Self {
        Self {
            found: Signal::new(HashMap::new()),
            events: Signal::new(HashMap::new()),
            proposals: Signal::new(Vec::new()),
            clashes: Signal::new(HashMap::new()),
            shares: Signal::new(HashMap::new()),
        }
    }
}

/// The thread's invitations, and what clashes with each, by message id.
/// Run on the loader thread.
pub fn read(
    commands: &Commands,
    thread: &Thread,
) -> (
    HashMap<String, Invitation>,
    HashMap<String, Vec<CalendarEvent>>,
) {
    let account = &thread.summary.account;
    let mut found = HashMap::new();
    let mut clashes = HashMap::new();
    for m in thread.messages.iter().filter(|m| may_invite(m)) {
        let Some(invitation) = commands.invitation(account, &m.id).ok().flatten() else {
            continue;
        };
        if invitation.kind == InvitationKind::Request {
            let clash: Vec<CalendarEvent> = commands
                .clashes(&invitation)
                .unwrap_or_default()
                .into_iter()
                .map(|h| h.event)
                .collect();
            clashes.insert(m.id.0.clone(), clash);
        }
        found.insert(m.id.0.clone(), invitation);
    }
    (found, clashes)
}

impl State {
    /// Look up each invitation the reader shows that asks for an answer,
    /// once, in its account's calendar.
    pub fn look_up_invitations(self) {
        Effect::new(move || {
            let found = self.invitations.found.get();
            let Some(thread) = self.thread.get() else {
                return;
            };
            let account = thread.summary.account.clone();
            // Only the open thread's are kept, so a thread opened again is
            // asked afresh: the person may have answered elsewhere since.
            untracked(|| {
                self.invitations
                    .events
                    .update(|e| e.retain(|id, _| found.contains_key(id)))
            });
            let asked = untracked(|| self.invitations.events.get());
            let wanted: Vec<(String, Invitation)> = found
                .into_iter()
                .filter(|(id, i)| i.kind == InvitationKind::Request && !asked.contains_key(id))
                .collect();
            if wanted.is_empty() {
                return;
            }
            let Some(handle) = sync() else {
                for (id, _) in wanted {
                    self.set_lookup(&id, Lookup::Failed(OFF.into()));
                }
                return;
            };
            for (id, invitation) in wanted {
                self.set_lookup(&id, Lookup::Looking);
                let (events, account, handle) =
                    (self.invitations.events, account.clone(), handle.clone());
                std::thread::spawn(move || {
                    let lookup = look_up(&handle, &account, &invitation);
                    events.update_send(move |e| {
                        e.insert(id, lookup);
                    });
                });
            }
        });
    }

    /// Answer the invitation in message `id` as `answer`, in its account's
    /// calendar, telling the organizer.
    pub fn answer_invitation(self, account: &EmailAddress, id: &str, answer: Answer) {
        let invitation = self.invitations.found.get().get(id).cloned();
        let lookup = self.invitations.events.get().get(id).cloned();
        let (Some(invitation), Some(Lookup::Found(event))) = (invitation, lookup) else {
            return;
        };
        if event.answer == Some(answer) {
            return;
        }
        let Some(handle) = sync() else {
            self.error.set(Some(OFF.into()));
            return;
        };
        self.set_lookup(id, Lookup::Answering(event.clone(), answer));
        let (events, error, revision) = (self.invitations.events, self.error, self.revision);
        let here = commands().map(|c| (*c).clone());
        let (account, id) = (account.clone(), id.to_owned());
        std::thread::spawn(move || {
            let sync = handle.sync_engine();
            let done =
                handle
                    .runtime
                    .block_on(sync.answer_invitation(&account, &invitation, answer));
            let (lookup, trouble) = match done {
                Ok(Answered::Done(event)) => {
                    // Given: an agent's answer waiting for it is settled.
                    if let Some(here) = &here {
                        settle(here, &account, &id);
                    }
                    revision.update_send(|n| *n += 1);
                    (Lookup::Found(*event), None)
                }
                Ok(Answered::NotInCalendar) => (Lookup::NotInCalendar, None),
                Ok(Answered::NotAGuest) => (
                    Lookup::Found(event),
                    Some(format!(
                        "{account} isn\u{2019}t among this event\u{2019}s guests, so there is nothing for it to answer."
                    )),
                ),
                Err(e) => (Lookup::Found(event), Some(say(&e))),
            };
            events.update_send(move |e| {
                e.insert(id, lookup);
            });
            if let Some(trouble) = trouble {
                error.send(Some(trouble));
            }
        });
    }

    /// Answer by mail the invitation in message `id`, which its account's
    /// calendar does not have.
    pub fn answer_by_mail(self, account: &EmailAddress, id: &str, answer: Answer) {
        let Some(here) = commands() else {
            return;
        };
        let message = ProviderMessageId(id.to_owned());
        if let Err(e) = here.answer_by_mail(account, &message, answer) {
            self.error.set(Some(e.to_string()));
            return;
        }
        settle(&here, account, id);
        self.set_lookup(id, Lookup::Mailed(answer));
        self.revision.update(|n| *n += 1);
        match sync() {
            Some(sync) => sync.send.notify_one(),
            None => self
                .error
                .set(Some("Saved here; it goes when sync is on.".to_owned())),
        }
    }

    /// Drop an agent's answer to the invitation in `message`, unanswered.
    pub fn dismiss_answer(self, account: &EmailAddress, message: &ProviderMessageId) {
        if let Some(here) = commands() {
            settle(&here, account, &message.0);
        }
        self.revision.update(|n| *n += 1);
    }

    /// Open the conversation an agent's answer is about.
    pub fn open_answer(self, account: &EmailAddress, proposal: &ProposedAnswer) {
        self.view.set(crate::state::View::Mail);
        self.open_row((account.clone(), proposal.thread.clone(), None));
    }

    /// The agent's answer waiting on the invitation in message `id`.
    pub fn proposed_answer(self, account: &EmailAddress, id: &str) -> Option<ProposedAnswer> {
        self.invitations.proposals.with(|all| {
            all.iter()
                .find(|(a, p)| a == account && p.message.0 == id)
                .map(|(_, p)| p.clone())
        })
    }

    fn set_lookup(self, id: &str, lookup: Lookup) {
        let id = id.to_owned();
        self.invitations.events.update(move |e| {
            e.insert(id, lookup);
        });
    }
}

fn look_up(handle: &SyncHandle, account: &EmailAddress, invitation: &Invitation) -> Lookup {
    let sync = handle.sync_engine();
    match handle
        .runtime
        .block_on(sync.invitation_event(account, invitation))
    {
        Ok(Some(event)) => Lookup::Found(event),
        Ok(None) => Lookup::NotInCalendar,
        Err(e) => {
            tracing::warn!("looking up an invitation in {account}'s calendar: {e}");
            Lookup::Failed(say(&e))
        }
    }
}

/// The agent's answer to the invitation in message `id` is settled.
fn settle(commands: &postrinch_commands::Commands, account: &EmailAddress, id: &str) {
    if let Err(e) = commands.settle_answer(account, &ProviderMessageId(id.to_owned())) {
        tracing::warn!("settling an agent's answer: {e}");
    }
}

/// A calendar failure in a sentence.
pub(crate) fn say(e: &postrinch_sync::SyncError) -> String {
    match e {
        postrinch_sync::SyncError::NeedsSignIn(_) => {
            "That account must sign in again in Postrinch first.".to_owned()
        }
        postrinch_sync::SyncError::Http(_) => {
            "Google Calendar could not be reached; try again when online.".to_owned()
        }
        other => format!("Google Calendar refused: {other}."),
    }
}
