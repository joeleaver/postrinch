//! What sends have shown about whether an IMAP account's provider files
//! the mail it sends in Sent itself ([`ImapSettings::server_files_sent`];
//! docs/imap.md, "Writing and sending").
//!
//! A provider said to file sent mail that has not filed a message by the
//! time a send has looked for it may be slow rather than not filing it at
//! all, so the setting goes off only after several such misses in a row,
//! and a copy found sets the count back to none. A provider whose setting
//! is off that files sent mail all the same shows it: the last message
//! Postrinch filed itself is then in Sent twice, and the setting comes
//! back on.
//!
//! Each change reads the account as it is now and writes back only the
//! setting and this state, under the account lock, so settings saved
//! meanwhile (the person's, or another send's) are never reverted.

use postrinch_model::{EmailAddress, ImapSettings, ProviderMessageId};
use serde::{Deserialize, Serialize};

use crate::codec::FieldsBuilder;
use crate::{Store, StoreError, StoreResult};

const TYPE: &str = "Account";

/// A kept copy marked deleted, which counts as gone.
const DELETED: &str = "\\Deleted";

/// The account's `sent_filing` field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SentFiling {
    /// Sends in a row whose look in Sent did not find the provider's copy.
    #[serde(default)]
    misses: u32,
    /// The last message Postrinch filed in Sent itself.
    #[serde(default)]
    filed: Option<ProviderMessageId>,
}

impl Store {
    /// Whether the provider files sent mail in Sent itself, as the
    /// account's settings say now. First, when the last message Postrinch
    /// filed itself has two live copies in `sent` (the Sent folder's label
    /// id, when sync keeps it), the provider filed it too: the setting is
    /// turned (or kept) on and the misses forgotten.
    pub fn server_files_sent(
        &self,
        address: &EmailAddress,
        sent: Option<&str>,
    ) -> StoreResult<bool> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (id, mut settings, mut filing) = self.sent_filing(address)?;
        let doubled = match (&filing.filed, sent) {
            (Some(message), Some(sent)) => {
                let copies = self
                    .imap_message_places(address, message)?
                    .into_iter()
                    .filter(|p| p.folder == sent && !p.flags.iter().any(|f| f == DELETED))
                    .count();
                copies >= 2
            }
            _ => false,
        };
        if doubled {
            settings.server_files_sent = true;
            filing = SentFiling::default();
            self.write_sent_filing(id, &settings, &filing)?;
        }
        Ok(settings.server_files_sent)
    }

    /// A send found the provider's own copy in Sent: the misses start
    /// again from none.
    pub fn sent_copy_found(&self, address: &EmailAddress) -> StoreResult<()> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (id, settings, mut filing) = self.sent_filing(address)?;
        if filing.misses != 0 {
            filing.misses = 0;
            self.write_sent_filing(id, &settings, &filing)?;
        }
        Ok(())
    }

    /// Postrinch filed `message` in Sent itself; `missed` when that was
    /// because the provider, said to file it, had not when the send
    /// looked. After `misses_allowed` such misses in a row the setting is
    /// turned off. Returns whether it was, by this call.
    pub fn sent_copy_filed(
        &self,
        address: &EmailAddress,
        message: &ProviderMessageId,
        missed: bool,
        misses_allowed: u32,
    ) -> StoreResult<bool> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (id, mut settings, mut filing) = self.sent_filing(address)?;
        filing.filed = Some(message.clone());
        let mut turned_off = false;
        if missed && settings.server_files_sent {
            filing.misses = filing.misses.saturating_add(1);
            if filing.misses >= misses_allowed {
                settings.server_files_sent = false;
                filing.misses = 0;
                turned_off = true;
            }
        }
        self.write_sent_filing(id, &settings, &filing)?;
        Ok(turned_off)
    }

    /// The account's id, its IMAP settings as stored now, and its state.
    fn sent_filing(&self, address: &EmailAddress) -> StoreResult<(u64, ImapSettings, SentFiling)> {
        let id = self.account_id(address)?;
        let fields = self.get(TYPE, id)?;
        let settings: ImapSettings = fields.opt_json("imap")?.ok_or_else(|| {
            StoreError::corrupt(TYPE, id, format!("{address} has no IMAP settings"))
        })?;
        let filing = fields.opt_json("sent_filing")?.unwrap_or_default();
        Ok((id, settings, filing))
    }

    fn write_sent_filing(
        &self,
        id: u64,
        settings: &ImapSettings,
        filing: &SentFiling,
    ) -> StoreResult<()> {
        let fields = FieldsBuilder::new()
            .json("imap", settings)
            .json("sent_filing", filing);
        self.db_update(TYPE, id, fields.build())
    }
}

#[cfg(test)]
#[path = "sent_filing_tests.rs"]
mod tests;
