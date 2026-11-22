//! Invitations in mail: the event a message's calendar part describes.

use postrinch_model::{
    Answer, EmailAddress, Invitation, InvitationKind, Message, ProposedAnswer, ProviderMessageId,
};

use crate::{CommandError, CommandResult, Commands};

/// Whether a message carries a calendar part, so may hold an invitation:
/// its parts as sync listed them, an inline `text/calendar` among them.
pub fn may_invite(message: &Message) -> bool {
    message.downloaded
        && message
            .attachments
            .iter()
            .any(|a| matches!(a.mime_type.as_str(), "text/calendar" | "application/ics"))
}

impl Commands {
    /// The invitation a downloaded message carries, its times read in the
    /// system's time zone where it names none. `None` when it carries none
    /// or is not downloaded.
    pub fn invitation(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
    ) -> CommandResult<Option<Invitation>> {
        self.check(account)?;
        let Some(raw) = self.store.raw_message(account, message)? else {
            return Ok(None);
        };
        Ok(postrinch_render::invitation(
            &raw,
            &jiff::tz::TimeZone::system(),
        ))
    }
}

impl Commands {
    /// Keep an agent's answer to the invitation in `message` for the person
    /// to give or not, in place of any earlier one.
    pub fn propose_answer(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
        answer: Answer,
        agent: &str,
        reason: Option<String>,
    ) -> CommandResult<ProposedAnswer> {
        self.check(account)?;
        let Some(found) = self.store.message(account, message)? else {
            return Err(CommandError::Invalid(format!(
                "{account} has no message {}.",
                message.0
            )));
        };
        let invitation = self
            .invitation(account, message)?
            .ok_or_else(|| CommandError::Invalid("That message carries no invitation.".into()))?;
        if invitation.kind != InvitationKind::Request {
            return Err(CommandError::Invalid(
                "That invitation asks for no answer: it is a cancellation, a guest's answer, \
                 or an event to add."
                    .into(),
            ));
        }
        let proposal = ProposedAnswer {
            message: message.clone(),
            thread: found.thread,
            answer,
            agent: agent.to_owned(),
            reason: reason.filter(|r| !r.trim().is_empty()),
            summary: invitation.summary,
            made: crate::views::now(),
        };
        self.store.put_proposed_answer(account, &proposal)?;
        Ok(proposal)
    }

    /// Every answer agents proposed, account by account, oldest first.
    pub fn answer_proposals(&self) -> CommandResult<Vec<(EmailAddress, ProposedAnswer)>> {
        let mut all = Vec::new();
        for account in self.visible_accounts()? {
            for p in self.store.proposed_answers(&account.address)? {
                all.push((account.address.clone(), p));
            }
        }
        Ok(all)
    }

    /// The person answered the invitation, or dropped the agent's answer.
    pub fn settle_answer(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
    ) -> CommandResult<()> {
        self.check(account)?;
        Ok(self.store.remove_proposed_answer(account, message)?)
    }
}

impl Commands {
    /// Answer by mail the invitation in `message`, which the account's
    /// calendar does not have: an iCalendar `REPLY` to its organizer, sent
    /// through the outbox like any message.
    pub fn answer_by_mail(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
        answer: Answer,
    ) -> CommandResult<()> {
        self.check(account)?;
        let raw = self
            .store
            .raw_message(account, message)?
            .ok_or_else(|| CommandError::Invalid("That message is not here yet.".into()))?;
        let invitation = self
            .invitation(account, message)?
            .filter(|i| i.kind == InvitationKind::Request)
            .ok_or_else(|| CommandError::Invalid("That message asks for no answer.".into()))?;
        let organizer = invitation.organizer.clone().ok_or_else(|| {
            CommandError::Invalid("The invitation names no organizer to answer.".into())
        })?;
        let ics = postrinch_render::invitation_reply(&raw, account, answer, crate::views::now())
            .ok_or_else(|| {
                CommandError::Invalid(format!(
                    "{account} isn\u{2019}t among the invitation\u{2019}s guests, so it has nothing to answer."
                ))
            })?;
        let found = self.store.message(account, message)?.ok_or_else(|| {
            CommandError::Invalid(format!("{account} has no message {}.", message.0))
        })?;
        let mut draft = self.start_draft(
            account,
            Some((&found.thread, message)),
            &crate::Start::Reply,
        )?;
        let (said, words) = match answer {
            Answer::Yes => ("Accepted", "has accepted"),
            Answer::Maybe => ("Tentatively accepted", "has tentatively accepted"),
            Answer::No => ("Declined", "has declined"),
        };
        let summary = invitation.summary.as_deref().unwrap_or("an event");
        draft.to = vec![organizer];
        draft.cc.clear();
        draft.bcc.clear();
        draft.subject = format!("{said}: {summary}");
        draft.body = format!("{account} {words} this invitation.\n");
        draft.attachments = vec![self.attach_bytes(
            "invite.ics",
            Some("text/calendar; method=REPLY"),
            ics.as_bytes(),
            postrinch_model::AttachmentOrigin::Person,
        )?];
        self.send_draft(&draft, None)
    }
}
