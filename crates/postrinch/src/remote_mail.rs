//! What agents and the window ask of an account's provider itself:
//! making, renaming and deleting labels (an IMAP account's folders), and
//! changing Google Calendar events and sharing, each through the
//! [`MailSync`] façade, which sends it to the account's provider. An IMAP
//! account has no calendars, and says so.
//!
//! Each call runs on sync's runtime and waits for it, so it is made from
//! a thread of its own, never the window's.

use postrinch_agents::Remote;
use postrinch_model::{CalendarInfo, CalendarShare, EmailAddress, EventDraft};
use postrinch_sync::{MailSync, SyncError, SyncResult};
use tokio::runtime::Handle;

use crate::sync::SyncHandle;

pub struct RemoteMail {
    runtime: Handle,
    mail: MailSync,
}

impl RemoteMail {
    pub fn new(runtime: Handle, mail: MailSync) -> Self {
        Self { runtime, mail }
    }

    /// Over the running sync.
    pub fn of(sync: &SyncHandle) -> Self {
        Self::new(sync.runtime.clone(), sync.mail().clone())
    }

    fn run<T>(&self, work: impl std::future::Future<Output = SyncResult<T>>) -> Result<T, String> {
        self.runtime.block_on(work).map_err(|e| {
            tracing::warn!("a change at the account's provider failed: {e}");
            said(e)
        })
    }
}

/// A failure as the person or agent reads it.
fn said(error: SyncError) -> String {
    match error {
        SyncError::NeedsSignIn(_) => "That account must sign in again in Postrinch first.".into(),
        SyncError::Http(_) | SyncError::Unreachable { .. } => {
            "The account's mail server could not be reached; try again when online.".into()
        }
        SyncError::Busy { message, .. } => {
            format!("The account's mail server is busy; try again later. It said: {message}")
        }
        SyncError::NoCalendars { .. } | SyncError::OAuthNotSetUp { .. } => {
            let mut sentence = error.to_string();
            sentence.push('.');
            sentence
        }
        other => format!("The account's mail server refused: {other}."),
    }
}

impl Remote for RemoteMail {
    fn create_label(&self, account: &EmailAddress, name: &str) -> Result<String, String> {
        Ok(self.run(self.mail.create_label(account, name))?.provider_id)
    }

    fn rename_label(&self, account: &EmailAddress, id: &str, name: &str) -> Result<(), String> {
        self.run(self.mail.rename_label(account, id, name))
            .map(drop)
    }

    fn delete_label(&self, account: &EmailAddress, id: &str) -> Result<(), String> {
        self.run(self.mail.delete_label(account, id))
    }

    fn save_event(
        &self,
        account: &EmailAddress,
        event: Option<&str>,
        draft: &EventDraft,
    ) -> Result<String, String> {
        self.run(async {
            let calendar = self.mail.calendar(account).await?;
            match event {
                Some(id) => calendar
                    .change_event(account, id, draft)
                    .await
                    .map(|()| id.to_owned()),
                None => calendar.create_event(account, draft).await,
            }
        })
    }

    fn delete_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        event: &str,
        tell: bool,
    ) -> Result<(), String> {
        self.run(async {
            self.mail
                .calendar(account)
                .await?
                .delete_event(account, calendar, event, tell)
                .await
        })
    }

    fn calendar_shares(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> Result<Vec<CalendarShare>, String> {
        self.run(async {
            self.mail
                .calendar(account)
                .await?
                .calendar_shares(account, calendar)
                .await
        })
    }

    fn add_calendar(&self, account: &EmailAddress, calendar: &str) -> Result<CalendarInfo, String> {
        self.run(async {
            self.mail
                .calendar(account)
                .await?
                .add_shared_calendar(account, calendar)
                .await
        })
    }
}

#[cfg(test)]
#[path = "remote_mail_tests.rs"]
mod tests;
