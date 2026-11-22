//! Invitations in mail: the event a message's calendar part describes.

use postrinch_model::{EmailAddress, Invitation, Message, ProviderMessageId};

use crate::{CommandResult, Commands};

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
