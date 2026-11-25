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
//! A provider that files sent mail, but only after the look gave up,
//! shows it the same way, by the copy Postrinch filed after a miss. That
//! is not a copy found in time, so the misses go on counting and the
//! setting still goes off. To keep such a provider from flipping the
//! setting back and forth, a second copy turns the setting back on only
//! once a back-off has passed since it last went off ([`SentBackOff`]:
//! an hour after the first time, doubling with each time after, at most
//! a week). A provider that was only slow for a while (an outage) so
//! gets its setting back within the hour; one that always files late
//! gets the waits back at most once a week. A copy found in time starts
//! the back-off again from an hour. Only copies under the folder's
//! UIDVALIDITY now count.
//!
//! Once the person sets the setting themselves (in the account's
//! settings: [`ImapSettings::server_files_sent_chosen`]), sends no longer
//! change it: neither misses nor a second copy override their choice.
//! They can hand it back to Postrinch, which then learns again from the
//! setting as it is ([`Store::choose_server_files_sent`]).
//!
//! Each change reads the account as it is now and writes back only the
//! setting and this state, under the account lock, so settings saved
//! meanwhile (the person's, or another send's) are never reverted.

use std::time::Duration;

use postrinch_model::{EmailAddress, ImapSettings, ProviderMessageId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::codec::FieldsBuilder;
use crate::{Store, StoreError, StoreResult};

const TYPE: &str = "Account";

/// A kept copy marked deleted, which counts as gone.
const DELETED: &str = "\\Deleted";

/// How long the setting, once Postrinch turned it off, stays off before a
/// provider seen to file sent mail all the same gets it back: `first`
/// after it first went off, twice as long each time after, and never more
/// than `longest`. A copy found in time starts again from `first`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentBackOff {
    pub first: Duration,
    pub longest: Duration,
}

impl Default for SentBackOff {
    /// An hour, then two, four and so on, at most a week.
    fn default() -> Self {
        Self {
            first: Duration::from_secs(60 * 60),
            longest: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

impl SentBackOff {
    /// The wait after the setting has gone off `flips` times since a copy
    /// was last found in time: none before it ever has.
    pub fn after(&self, flips: u32) -> Duration {
        let Some(doublings) = flips.checked_sub(1) else {
            return Duration::ZERO;
        };
        1u32.checked_shl(doublings)
            .and_then(|factor| self.first.checked_mul(factor))
            .map_or(self.longest, |wait| wait.min(self.longest))
    }
}

/// The account's `sent_filing` field. A field kept by earlier versions
/// (`after_miss`, `late`) is no longer read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SentFiling {
    /// Sends in a row whose look in Sent did not find the provider's copy.
    #[serde(default)]
    misses: u32,
    /// The last message Postrinch filed in Sent itself.
    #[serde(default)]
    filed: Option<ProviderMessageId>,
    /// How many times Postrinch has turned the setting off since a copy
    /// was last found in time.
    #[serde(default)]
    flips: u32,
    /// When the wait for the setting to come back on started: when
    /// Postrinch last turned it off, or when a second copy was first seen
    /// while it was off with no such time (the person turned it off).
    #[serde(default)]
    off_since: Option<Timestamp>,
}

impl Store {
    /// Whether the provider files sent mail in Sent itself, as the
    /// account's settings say now. First, when the last message Postrinch
    /// filed itself has two live copies in `sent` (the Sent folder's label
    /// id when sync keeps it, and its UIDVALIDITY now), the provider filed
    /// it too: a setting that is off is turned back on, and the misses
    /// forgotten, once `back_off` has passed since Postrinch turned it off
    /// (at `now`). A setting that is on stays on, and the misses go on: a
    /// copy filed after a miss is not one found in time.
    pub fn server_files_sent(
        &self,
        address: &EmailAddress,
        sent: Option<(&str, u32)>,
        back_off: &SentBackOff,
        now: Timestamp,
    ) -> StoreResult<bool> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (id, mut settings, mut filing) = self.sent_filing(address)?;
        if settings.server_files_sent_chosen {
            return Ok(settings.server_files_sent);
        }
        let doubled = match (&filing.filed, sent) {
            (Some(message), Some((sent, uidvalidity))) => {
                let copies = self
                    .imap_message_places(address, message)?
                    .into_iter()
                    .filter(|p| {
                        p.folder == sent
                            && p.uidvalidity == uidvalidity
                            && !p.flags.iter().any(|f| f == DELETED)
                    })
                    .count();
                copies >= 2
            }
            _ => false,
        };
        if !doubled {
            return Ok(settings.server_files_sent);
        }
        filing.filed = None;
        if !settings.server_files_sent {
            // Off with no time kept (the person handed back a setting
            // they had turned off), or a clock set back since: the wait
            // starts now.
            let since = match filing.off_since {
                Some(since) if since <= now => since,
                _ => {
                    filing.off_since = Some(now);
                    now
                }
            };
            let waited = u64::try_from(now.as_millis() - since.as_millis()).unwrap_or(0);
            if Duration::from_millis(waited) >= back_off.after(filing.flips) {
                settings.server_files_sent = true;
                filing.misses = 0;
                filing.off_since = None;
            }
        }
        self.write_sent_filing(id, &settings, &filing)?;
        Ok(settings.server_files_sent)
    }

    /// A send found the provider's own copy in Sent, in time: the misses
    /// start again from none, and so does the back-off.
    pub fn sent_copy_found(&self, address: &EmailAddress) -> StoreResult<()> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (id, settings, mut filing) = self.sent_filing(address)?;
        if filing.misses != 0 || filing.flips != 0 {
            filing.misses = 0;
            filing.flips = 0;
            self.write_sent_filing(id, &settings, &filing)?;
        }
        Ok(())
    }

    /// Postrinch filed `message` in Sent itself (at `now`); `missed` when
    /// that was because the provider, said to file it, had not when the
    /// send looked. After `misses_allowed` such misses in a row the
    /// setting is turned off, and the back-off grows. Returns whether it
    /// was, by this call.
    pub fn sent_copy_filed(
        &self,
        address: &EmailAddress,
        message: &ProviderMessageId,
        missed: bool,
        misses_allowed: u32,
        now: Timestamp,
    ) -> StoreResult<bool> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (id, mut settings, mut filing) = self.sent_filing(address)?;
        filing.filed = Some(message.clone());
        let mut turned_off = false;
        if missed && settings.server_files_sent && !settings.server_files_sent_chosen {
            filing.misses = filing.misses.saturating_add(1);
            if filing.misses >= misses_allowed {
                settings.server_files_sent = false;
                filing.misses = 0;
                filing.flips = filing.flips.saturating_add(1);
                filing.off_since = Some(now);
                turned_off = true;
            }
        }
        self.write_sent_filing(id, &settings, &filing)?;
        Ok(turned_off)
    }

    /// The person sets whether the provider files sent mail in Sent
    /// itself (`Some`), and sends no longer change it; or hands it back
    /// to Postrinch (`None`), which learns again from the setting as it
    /// is. Either way, what sends had shown so far is forgotten. Returns
    /// the settings as stored.
    pub fn choose_server_files_sent(
        &self,
        address: &EmailAddress,
        choice: Option<bool>,
    ) -> StoreResult<ImapSettings> {
        let _held = self.account_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (id, mut settings, _) = self.sent_filing(address)?;
        match choice {
            Some(on) => {
                settings.server_files_sent = on;
                settings.server_files_sent_chosen = true;
            }
            None => settings.server_files_sent_chosen = false,
        }
        self.write_sent_filing(id, &settings, &SentFiling::default())?;
        Ok(settings)
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
