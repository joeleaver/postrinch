//! Delivering an IMAP account's outgoing mail, in the order it was made
//! (docs/imap.md, "Writing and sending"): messages go over SMTP and are
//! filed in Sent ([`sent`]), drafts are kept in Drafts ([`drafts`]).
//!
//! Each message goes once. SMTP cannot be asked whether a message got
//! through, so how far an attempt got is kept with the message
//! ([`SendStage`]) and decides what the next one does: a message whose
//! data reached the server without an answer is looked for in Sent, never
//! sent blind; one the server took is only filed, never sent again.

mod drafts;
mod mailbox;
mod sent;
mod submit;

use std::time::Duration;

use postrinch_model::{EmailAddress, ImapSettings, ProviderMessageId};
use postrinch_store::{Outgoing, OutgoingKind, SendStage};

use super::{ImapSync, Lane};
use crate::compose::outgoing_message_id;
use crate::imap::message_id;
use crate::{SyncError, SyncResult, build_message};

/// How a send looks in Sent for a message the provider files itself: at
/// once, then after each of `waits` (by default 1, 2, 4 and 8 seconds, 15
/// in all), since a provider may take a moment to file it. A message the
/// provider does not file is looked for once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentLookup {
    pub waits: Vec<Duration>,
}

impl Default for SentLookup {
    fn default() -> Self {
        Self {
            waits: [1, 2, 4, 8].map(Duration::from_secs).to_vec(),
        }
    }
}

/// What became of one piece of outgoing mail, short of a failure to try
/// again.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Delivery {
    /// Sent (and filed), or kept: it is done with.
    Done,
    /// Refused for good, in these words: it waits for the person.
    Refused(String),
    /// Its data reached the server, no answer came, and it is not in
    /// Sent: it waits for the person, who may send it again.
    MayHaveGone(String),
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
}

impl ImapSync {
    /// Delivers the account's waiting mail, oldest first: messages are
    /// sent, drafts kept. Mail refused for good, or that may have gone,
    /// is marked so and waits for the person; any other failure stops
    /// here, for the next call, and is returned (a server too busy is
    /// [`SyncError::Busy`]; a refused sign-in also marks the account as
    /// needing to sign in). Returns how many were delivered.
    pub async fn send_outgoing(&self, account: &EmailAddress) -> SyncResult<usize> {
        let owner = account.clone();
        let waiting = self.with_store(move |s| s.outgoing_for(&owner)).await?;
        let mut delivered = 0;
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
        Ok(delivered)
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
        let built = match self.build(outgoing, from_name.as_deref()).await? {
            Ok(built) => built,
            Err(why) => return Ok(Delivery::Refused(why)),
        };
        match outgoing.kind {
            OutgoingKind::Send => {
                self.send_message(account, &settings, outgoing, &built)
                    .await
            }
            OutgoingKind::Keep => {
                let replaces = outgoing.draft.replaces.as_ref();
                self.over(account, Lane::Outgoing, async |conn| {
                    self.keep_draft(conn, account, &settings, &built, replaces)
                        .await
                })
                .await
            }
            OutgoingKind::Proposed => Ok(Delivery::Skipped),
        }
    }

    /// The message a queued draft makes, or why it cannot be made (an
    /// attachment gone from this computer: the person must take it out
    /// or attach it again).
    async fn build(
        &self,
        outgoing: &Outgoing,
        from_name: Option<&str>,
    ) -> SyncResult<Result<Built, String>> {
        let header = outgoing_message_id(outgoing);
        let wanted = outgoing.draft.attachments.clone();
        let files = self
            .with_store(move |s| {
                wanted
                    .iter()
                    .map(|a| s.attachment_bytes(&a.blob).map(|b| (a.name.clone(), b)))
                    .collect::<Result<Vec<_>, _>>()
            })
            .await?;
        let mut bytes = Vec::with_capacity(files.len());
        for (name, file) in files {
            let Some(file) = file else {
                return Ok(Err(format!("{name} is no longer on this computer")));
            };
            bytes.push(file);
        }
        let raw = build_message(&outgoing.draft, from_name, &header, &bytes);
        Ok(Ok(Built {
            id: message_id(&outgoing.draft.account, &header),
            header,
            raw,
        }))
    }

    /// Records how far sending a message got.
    async fn set_stage(&self, id: u64, stage: SendStage) -> SyncResult<()> {
        self.with_store(move |s| s.outgoing_stage(id, stage)).await
    }

    /// How a send looks for a message in Sent: with the waits when the
    /// provider files sent mail itself, otherwise once.
    fn sent_waits(&self, settings: &ImapSettings) -> &[Duration] {
        if settings.server_files_sent {
            &self.sent_lookup.waits
        } else {
            &[]
        }
    }
}
