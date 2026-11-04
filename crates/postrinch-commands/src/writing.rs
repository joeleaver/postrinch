//! Writing mail: drafts to start from (new, reply, reply to all, forward,
//! or a draft kept earlier), and what becomes of them: sent, kept as a
//! draft, or proposed by an agent for the person to send.
//!
//! Sending needs the person: the window's Send is their say. An agent can
//! only propose (DESIGN.md "Safety"), and a proposal shows in the window
//! until the person sends, keeps or discards it.

use std::collections::HashSet;

use postrinch_model::{
    Answering, Draft, EmailAddress, Mailbox, Message, ProviderMessageId, ProviderThreadId,
    SystemLabel, Timestamp,
};
use postrinch_store::{Outgoing, OutgoingKind};
use serde::{Deserialize, Serialize};

use crate::{CommandError, CommandResult, Commands};

/// What to start writing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Start {
    New,
    Reply,
    ReplyAll,
    Forward,
}

/// Mail on its way out, as the window shows it.
pub type Queued = Outgoing;

impl Commands {
    /// A draft to start from: new mail from `account`, or an answer to (or
    /// forward of) `message` in `thread`.
    pub fn start_draft(
        &self,
        account: &EmailAddress,
        answering: Option<(&ProviderThreadId, &ProviderMessageId)>,
        start: &Start,
    ) -> CommandResult<Draft> {
        self.check(account)?;
        let mut draft = Draft::new(account.clone());
        let Some((thread, id)) = answering else {
            return Ok(draft);
        };
        let messages = self.store.thread_messages(account, thread)?;
        let Some(message) = messages.iter().find(|m| &m.id == id) else {
            return Ok(draft);
        };
        let h = &message.headers;
        let subject = h.subject.clone().unwrap_or_default();
        match start {
            Start::New => {}
            Start::Reply | Start::ReplyAll => {
                draft.to = if h.reply_to.is_empty() {
                    h.from.clone()
                } else {
                    h.reply_to.clone()
                };
                if *start == Start::ReplyAll {
                    let mut seen: HashSet<String> =
                        draft.to.iter().map(|m| m.address.key()).collect();
                    seen.insert(account.key());
                    draft.cc =
                        h.to.iter()
                            .chain(&h.cc)
                            .filter(|m| seen.insert(m.address.key()))
                            .cloned()
                            .collect();
                }
                // Answering what you sent goes to whom you sent it to.
                if draft.to.iter().all(|m| m.address.key() == account.key()) {
                    draft.to = h.to.clone();
                }
                draft.subject = prefixed("Re:", &subject);
                draft.body = format!("\n\n{}", quote(message));
                let mut references = h.references.clone();
                references.extend(h.message_id.clone());
                draft.answering = Answering {
                    thread: Some(thread.clone()),
                    in_reply_to: h.message_id.clone(),
                    references,
                };
            }
            Start::Forward => {
                draft.subject = prefixed("Fwd:", &subject);
                draft.body = format!("\n\n{}", forwarded(message));
                draft.answering.thread = Some(thread.clone());
            }
        }
        Ok(draft)
    }

    /// A draft kept with the provider, to carry on writing: sending or
    /// keeping it again replaces it.
    pub fn edit_draft(&self, account: &EmailAddress, message: &Message) -> CommandResult<Draft> {
        self.check(account)?;
        let h = &message.headers;
        let mut draft = Draft::new(account.clone());
        draft.to = h.to.clone();
        draft.cc = h.cc.clone();
        draft.bcc = h.bcc.clone();
        draft.subject = h.subject.clone().unwrap_or_default();
        draft.body = message.body_text.clone();
        draft.answering = Answering {
            thread: Some(message.thread.clone()),
            in_reply_to: h.in_reply_to.clone(),
            references: h.references.clone(),
        };
        draft.replaces = Some(message.id.clone());
        Ok(draft)
    }

    /// Send a draft: queued here at once, delivered by sync. `settles` is
    /// the proposal it came from, if an agent wrote it.
    pub fn send_draft(&self, draft: &Draft, settles: Option<u64>) -> CommandResult<()> {
        self.check(&draft.account)?;
        if draft.recipients().next().is_none() {
            return Err(CommandError::Invalid("Add someone to send it to.".into()));
        }
        self.store.queue_outgoing(draft, OutgoingKind::Send)?;
        self.settle(settles)
    }

    /// Keep a draft with the provider, to finish later.
    pub fn keep_draft(&self, draft: &Draft, settles: Option<u64>) -> CommandResult<()> {
        self.check(&draft.account)?;
        self.store.queue_outgoing(draft, OutgoingKind::Keep)?;
        self.settle(settles)
    }

    /// An agent's draft, for the person to send: nothing leaves until they
    /// do. Returns the proposal's id.
    pub fn propose_draft(&self, draft: &Draft) -> CommandResult<u64> {
        self.check(&draft.account)?;
        if draft.proposal.is_none() {
            return Err(CommandError::Invalid("A proposal names its agent.".into()));
        }
        Ok(self.store.queue_outgoing(draft, OutgoingKind::Proposed)?)
    }

    /// Discard a proposal (or forget one the person dealt with).
    pub fn discard_proposal(&self, id: u64) -> CommandResult<()> {
        self.settle(Some(id))
    }

    /// Mail on its way out, and proposals waiting for the person, oldest
    /// first, for the accounts this caller sees.
    pub fn outgoing(&self) -> CommandResult<Vec<Queued>> {
        let mut all = self.store.outgoing()?;
        all.retain(|o| self.sees(&o.draft.account));
        Ok(all)
    }

    fn settle(&self, proposal: Option<u64>) -> CommandResult<()> {
        if let Some(id) = proposal
            && self
                .outgoing()?
                .iter()
                .any(|o| o.id == id && o.kind == OutgoingKind::Proposed)
        {
            self.store.outgoing_done(id)?;
        }
        Ok(())
    }

    /// Whether a message is one of the provider's drafts.
    pub fn is_draft(&self, account: &EmailAddress, message: &Message) -> CommandResult<bool> {
        let labels = self.store.labels(account)?;
        Ok(labels.iter().any(|l| {
            l.kind == postrinch_model::LabelKind::System(SystemLabel::Drafts)
                && message.labels.contains(&l.provider_id)
        }))
    }
}

/// `Re: subject`, once.
fn prefixed(prefix: &str, subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        trimmed.to_owned()
    } else if trimmed.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix} {trimmed}")
    }
}

fn name(message: &Message) -> String {
    message
        .headers
        .from
        .first()
        .map_or_else(|| "someone".to_owned(), Mailbox::display_name)
}

fn when(at: Timestamp) -> String {
    jiff::Timestamp::from_millisecond(at.as_millis())
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%a %-d %b %Y at %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// The answered message, quoted under a line saying who wrote it when.
fn quote(message: &Message) -> String {
    let quoted: Vec<String> = message
        .body_text
        .trim_end()
        .lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_owned()
            } else {
                format!("> {l}")
            }
        })
        .collect();
    format!(
        "On {}, {} wrote:\n{}",
        when(message.received),
        name(message),
        quoted.join("\n")
    )
}

/// The forwarded message, under its headers.
fn forwarded(message: &Message) -> String {
    let h = &message.headers;
    let list = |m: &[Mailbox]| {
        m.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut head = vec![
        "---------- Forwarded message ----------".to_owned(),
        format!("From: {}", list(&h.from)),
        format!("Date: {}", when(message.received)),
        format!("Subject: {}", h.subject.clone().unwrap_or_default()),
        format!("To: {}", list(&h.to)),
    ];
    if !h.cc.is_empty() {
        head.push(format!("Cc: {}", list(&h.cc)));
    }
    format!("{}\n\n{}", head.join("\n"), message.body_text.trim_end())
}
