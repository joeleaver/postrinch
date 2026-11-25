//! Delivering an IMAP account's outgoing mail, in the order it was made
//! (docs/imap.md, "Writing and sending"): messages go over SMTP and are
//! filed in Sent ([`sent`]), drafts are kept in Drafts ([`drafts`]).
//!
//! Each message goes once. SMTP cannot be asked whether a message got
//! through, so how far an attempt got is kept with the message
//! ([`SendStage`]) and decides what the next one does: a message whose
//! data reached the server without an answer is looked for in Sent, never
//! sent blind; one the server took is only filed, never sent again.
//!
//! One delivery per account at a time: [`ImapSync::send_outgoing`] holds
//! the account's sending lock while it works, so two callers never take
//! the same message; the second waits, then finds what is left.

mod drafts;
mod mailbox;
mod put_off;
mod sent;
mod submit;
mod transmit;

use std::time::Duration;

use postrinch_model::{EmailAddress, ProviderMessageId};
use postrinch_store::{Outgoing, OutgoingKind, SentBackOff};

use super::{ImapSync, Lane};
use crate::compose::outgoing_message_id;
use crate::imap::message_id;
use crate::{SyncError, SyncResult, build_message};

/// How a send looks in Sent for a message the provider files itself: at
/// once, then after each of `waits` (by default 1, 2, 4 and 8 seconds, 15
/// in all), since a provider may take a moment to file it. A message the
/// provider does not file is looked for once. Once Postrinch has turned
/// the provider's setting off, a provider seen to file sent mail all the
/// same gets it back only after `back_off` (by default an hour, doubling
/// each time it goes off again, at most a week).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentLookup {
    pub waits: Vec<Duration>,
    pub back_off: SentBackOff,
}

impl Default for SentLookup {
    fn default() -> Self {
        Self {
            waits: [1, 2, 4, 8].map(Duration::from_secs).to_vec(),
            back_off: SentBackOff::default(),
        }
    }
}

/// How many sends in a row may look in Sent for the copy a provider said
/// to file sent mail makes, and not find it, before Postrinch files sent
/// mail itself: one slow lookup is not enough to tell.
const SENT_MISSES_ALLOWED: u32 = 3;

/// What became of one piece of outgoing mail, short of a failure to try
/// again.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Delivery {
    /// Sent (and filed), or kept: it is done with.
    Done,
    /// Refused for good, in these words: it waits for the person.
    Refused(String),
    /// Its data reached the server, no answer came, and it is not in
    /// Sent (or the server took it and recording that failed): it waits
    /// for the person, who may send it again.
    MayHaveGone(String),
    /// It went to everyone but some recipients the server put off for
    /// now (described in `why`): they are tried again later.
    PutOff { server: String, why: String },
    /// Not the provider's (an agent's proposal).
    Skipped,
}

/// A message as it goes out.
struct Built {
    /// Its `Message-ID`, without brackets.
    header: String,
    /// The provider id that `Message-ID` gives it.
    id: ProviderMessageId,
    /// Its bytes, `Bcc` included (as Sent and Drafts keep it).
    raw: Vec<u8>,
    /// The attachments no longer on this computer, by name: left out of
    /// `raw`, which then goes nowhere but Sent.
    missing: Vec<String>,
}

impl Built {
    /// Why it cannot go out as the person made it: an attachment gone
    /// from this computer (the person must take it out or attach it
    /// again).
    fn incomplete(&self) -> Option<String> {
        let name = self.missing.first()?;
        Some(format!("{name} is no longer on this computer"))
    }
}

impl ImapSync {
    /// Delivers the account's waiting mail, oldest first: messages are
    /// sent, drafts kept. Mail refused for good, or that may have gone,
    /// is marked so and waits for the person; any other failure stops
    /// here, for the next call, and is returned (a server too busy is
    /// [`SyncError::Busy`]; a refused sign-in also marks the account as
    /// needing to sign in). A message the server put some recipients off
    /// for does not hold up the rest: once they are delivered, that is
    /// returned as [`SyncError::Busy`], for the next call to try them
    /// again. Returns how many were delivered.
    ///
    /// Calls for one account take turns, so a message never goes twice
    /// because two deliveries read it waiting at once.
    pub async fn send_outgoing(&self, account: &EmailAddress) -> SyncResult<usize> {
        let sending = self.sending(account);
        let _turn = sending.lock().await;
        let owner = account.clone();
        let waiting = self.with_store(move |s| s.outgoing_for(&owner)).await?;
        let mut delivered = 0;
        let mut put_off = None;
        for outgoing in waiting.into_iter().filter(|o| !o.refused) {
            let id = outgoing.id;
            match self.deliver(account, &outgoing).await {
                Ok(Delivery::Done) => {
                    self.with_store(move |s| s.outgoing_done(id)).await?;
                    delivered += 1;
                }
                Ok(Delivery::Refused(why) | Delivery::MayHaveGone(why)) => {
                    tracing::warn!("IMAP: {account}: outgoing mail waits for the person: {why}");
                    self.with_store(move |s| s.outgoing_failed(id, &why, true))
                        .await?;
                }
                Ok(Delivery::PutOff { server, why }) => {
                    tracing::info!("IMAP: {account}: outgoing mail is tried again later: {why}");
                    self.with_store({
                        let why = why.clone();
                        move |s| s.outgoing_failed(id, &why, false)
                    })
                    .await?;
                    put_off.get_or_insert(SyncError::Busy {
                        service: "SMTP",
                        server,
                        message: why,
                    });
                }
                Ok(Delivery::Skipped) => {}
                Err(e) => {
                    let error = e.to_string();
                    self.with_store(move |s| s.outgoing_failed(id, &error, false))
                        .await?;
                    let failed = Err(e);
                    self.note_sign_in(account, &failed).await?;
                    return failed;
                }
            }
        }
        match put_off {
            Some(e) => Err(e),
            None => Ok(delivered),
        }
    }

    async fn deliver(&self, account: &EmailAddress, outgoing: &Outgoing) -> SyncResult<Delivery> {
        if outgoing.kind == OutgoingKind::Proposed {
            return Ok(Delivery::Skipped);
        }
        // Read for each message: a send may have changed them.
        let owner = account.clone();
        let stored = self.with_store(move |s| s.account(&owner)).await?;
        let from_name = stored.as_ref().and_then(|a| a.display_name.clone());
        let settings = stored
            .and_then(|a| a.imap)
            .ok_or_else(|| SyncError::Config(format!("{account} is not an IMAP account")))?;
        let built = self.build(outgoing, from_name.as_deref()).await?;
        let replaced = self
            .replaced(account, outgoing.draft.replaces.as_ref())
            .await?;
        match outgoing.kind {
            OutgoingKind::Send => {
                self.send_message(account, &settings, outgoing, &built, &replaced)
                    .await
            }
            OutgoingKind::Keep => {
                // Kept without an attachment, the draft would lose it.
                if let Some(why) = built.incomplete() {
                    return Ok(Delivery::Refused(why));
                }
                self.over(account, Lane::Outgoing, async |conn| {
                    self.keep_draft(conn, account, &settings, &built, &replaced)
                        .await
                })
                .await
            }
            OutgoingKind::Proposed => Ok(Delivery::Skipped),
        }
    }

    /// The message a queued draft makes, without the attachments no
    /// longer on this computer ([`Built::missing`]): such a message is
    /// only filed (one already delivered), never sent or kept.
    async fn build(&self, outgoing: &Outgoing, from_name: Option<&str>) -> SyncResult<Built> {
        let header = outgoing_message_id(outgoing);
        let wanted = outgoing.draft.attachments.clone();
        let files = self
            .with_store(move |s| {
                wanted
                    .iter()
                    .map(|a| s.attachment_bytes(&a.blob))
                    .collect::<Result<Vec<_>, _>>()
            })
            .await?;
        let mut draft = outgoing.draft.clone();
        let mut bytes = Vec::with_capacity(files.len());
        let mut missing = Vec::new();
        let mut present = Vec::with_capacity(files.len());
        for (attachment, file) in outgoing.draft.attachments.iter().zip(files) {
            match file {
                Some(file) => {
                    bytes.push(file);
                    present.push(attachment.clone());
                }
                None => missing.push(attachment.name.clone()),
            }
        }
        draft.attachments = present;
        let raw = build_message(&draft, from_name, &header, &bytes);
        Ok(Built {
            id: message_id(&outgoing.draft.account, &header),
            header,
            raw,
            missing,
        })
    }

    /// How a send looks for a message in Sent: with the waits when the
    /// provider files sent mail itself, otherwise once.
    fn sent_waits(&self, server_files_sent: bool) -> &[Duration] {
        if server_files_sent {
            &self.sent_lookup.waits
        } else {
            &[]
        }
    }
}
