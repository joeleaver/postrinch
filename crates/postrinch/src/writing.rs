//! Writing mail from the window: what the Write button, Reply, Reply all,
//! Forward and Edit draft open, and what Send, Keep as draft and Discard
//! do. The command layer builds and queues drafts; sync's sender delivers
//! them (see `postrinch-commands`' writing.rs).

use postrinch_commands::{Commands, Queued, Start, Thread};
use postrinch_model::{Draft, EmailAddress, Message, ProviderMessageId, ProviderThreadId};

use crate::actions::{commands, sync};
use crate::state::State;

/// The draft the compose dialog has open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compose {
    pub draft: Draft,
    /// The agent's proposal it came from, settled when the person acts.
    pub proposal: Option<u64>,
    pub title: String,
    /// Counts openings, so each starts the dialog fresh.
    pub opening: u64,
}

impl State {
    /// Open the compose dialog on `draft`.
    fn compose(self, draft: Draft, proposal: Option<u64>, title: String) {
        let opening = self
            .composing
            .with(|c| c.as_ref().map_or(0, |c| c.opening + 1));
        self.compose_error.set(None);
        self.composing.set(Some(Compose {
            draft,
            proposal,
            title,
            opening,
        }));
    }

    /// Write: new mail, or an answer to the open conversation's latest
    /// message.
    pub fn write(self, start: Start) {
        self.answer(start, self.open_message_id().as_ref());
    }

    /// Write new mail, or answer `message` in the open conversation (its
    /// latest downloaded message when `None`).
    pub fn answer(self, start: Start, message: Option<&ProviderMessageId>) {
        let Some(commands) = commands() else {
            return;
        };
        match (&start, self.thread.get()) {
            (Start::New, _) => match self.writing_account() {
                Some(account) => self.start_draft(&commands, &account, None, start),
                None => self.error.set(Some("Add an account to write from.".into())),
            },
            (_, Some(thread)) => self.answer_in(&commands, start, &thread, message),
            (_, None) => {}
        }
    }

    /// Answer a row of the list, open or not: its one message when mail is
    /// listed a message at a time, else its conversation's latest.
    pub fn answer_row(self, start: Start, row: &crate::ui::list::RowId) {
        let Some(commands) = commands() else {
            return;
        };
        match commands.read_thread(&row.0, &row.1) {
            Ok(Some(thread)) => self.answer_in(&commands, start, &thread, row.2.as_ref()),
            Ok(None) => {}
            Err(e) => self.error.set(Some(e.to_string())),
        }
    }

    /// Answer `message` of `thread`, or its latest downloaded message.
    fn answer_in(
        self,
        commands: &Commands,
        start: Start,
        thread: &Thread,
        message: Option<&ProviderMessageId>,
    ) {
        let chosen = match message {
            Some(id) => thread.messages.iter().find(|m| &m.id == id),
            None => thread
                .messages
                .iter()
                .rev()
                .find(|m| m.downloaded)
                .or(thread.messages.last()),
        };
        let Some(chosen) = chosen else {
            return;
        };
        let answering = (&thread.summary.id, &chosen.id);
        self.start_draft(commands, &thread.summary.account, Some(answering), start);
    }

    fn start_draft(
        self,
        commands: &Commands,
        account: &EmailAddress,
        answering: Option<(&ProviderThreadId, &ProviderMessageId)>,
        start: Start,
    ) {
        match commands.start_draft(account, answering, &start) {
            Ok(draft) => {
                let title = match start {
                    Start::New => "New message".to_owned(),
                    Start::Reply => format!("Reply: {}", topic(&draft)),
                    Start::ReplyAll => format!("Reply to all: {}", topic(&draft)),
                    Start::Forward => format!("Forward: {}", topic(&draft)),
                };
                self.compose(draft, None, title);
            }
            Err(e) => self.error.set(Some(e.to_string())),
        }
    }

    /// Carry on writing one of the provider's drafts.
    pub fn edit_draft(self, message: &Message) {
        let Some(commands) = commands() else {
            return;
        };
        match commands.edit_draft(&message.account, message) {
            Ok(draft) => self.compose(draft, None, "Draft".to_owned()),
            Err(e) => self.error.set(Some(e.to_string())),
        }
    }

    /// Open an agent's proposal for the person to check and send.
    pub fn open_proposal(self, queued: &Queued) {
        let agent = queued
            .draft
            .proposal
            .as_ref()
            .map_or_else(|| "An agent".to_owned(), |p| p.agent.clone());
        let title = format!("{agent} wrote this and wants to send it");
        self.compose(queued.draft.clone(), Some(queued.id), title);
    }

    /// Send what the dialog holds.
    pub fn send_composed(self, draft: Draft) {
        self.finish(|c, proposal| c.send_draft(&draft, proposal));
    }

    /// Keep what the dialog holds as a draft with the provider.
    pub fn keep_composed(self, draft: Draft) {
        self.finish(|c, proposal| c.keep_draft(&draft, proposal));
    }

    /// Close the dialog without sending; an agent's proposal goes too.
    pub fn discard_composed(self) {
        if let Some(proposal) = self.composing.get().and_then(|c| c.proposal)
            && let Some(commands) = commands()
            && let Err(e) = commands.discard_proposal(proposal)
        {
            tracing::warn!("discarding a proposal: {e}");
        }
        self.composing.set(None);
        self.revision.update(|n| *n += 1);
    }

    fn finish(
        self,
        act: impl FnOnce(
            &postrinch_commands::Commands,
            Option<u64>,
        ) -> postrinch_commands::CommandResult<()>,
    ) {
        let Some(commands) = commands() else {
            return;
        };
        let proposal = self.composing.get().and_then(|c| c.proposal);
        match act(&commands, proposal) {
            Ok(()) => {
                self.composing.set(None);
                self.revision.update(|n| *n += 1);
                match sync() {
                    Some(sync) => sync.send.notify_one(),
                    None => self
                        .error
                        .set(Some("Saved here; it goes when sync is on.".to_owned())),
                }
            }
            Err(e) => self.compose_error.set(Some(e.to_string())),
        }
    }

    /// The account new mail goes from: the one being looked at, else the
    /// first.
    fn writing_account(self) -> Option<postrinch_model::EmailAddress> {
        if let Some(account) = self.shown_account() {
            return Some(account);
        }
        self.sidebar.with(|s| {
            s.as_ref()
                .and_then(|s| s.accounts.first().map(|a| a.account.address.clone()))
        })
    }
}

fn topic(draft: &Draft) -> String {
    let subject = draft.subject.trim();
    subject
        .strip_prefix("Re: ")
        .or_else(|| subject.strip_prefix("Fwd: "))
        .unwrap_or(subject)
        .to_owned()
}
