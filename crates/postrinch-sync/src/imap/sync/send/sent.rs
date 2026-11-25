//! Sending a message and filing it in Sent (docs/imap.md, "Writing and
//! sending").
//!
//! How far an attempt got is recorded with the message ([`SendStage`]):
//! `DataSent` just before its data goes, `Delivered` once the server took
//! it. A message whose data went without an answer (the connection lost,
//! or the app stopped) is looked for in Sent by its `Message-ID`; found,
//! it was delivered, otherwise it waits for the person, as it may have
//! gone. A delivered message is filed in Sent (APPEND, `\Seen`) unless
//! the provider files it itself; a provider said to, that has not a
//! moment later, gets the copy and its setting turned off. Then the draft
//! it came from is deleted.

use std::time::Duration;

use postrinch_model::{EmailAddress, ImapSettings, SystemLabel};
use postrinch_store::{Outgoing, SendStage};

use super::super::places::SEEN;
use super::super::wire::Conn;
use super::mailbox::{self, Appended, Target};
use super::submit::{self, Submitted};
use super::{Built, Delivery};
use crate::SyncResult;
use crate::imap::SmtpConnection;
use crate::imap::sync::{ImapSync, Lane};

/// How a delivered message is filed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filing {
    /// Just delivered: appended, unless the provider files it.
    New,
    /// Delivered on an earlier attempt that did not finish: looked for
    /// first, as that attempt may have filed it.
    Again,
    /// Found in Sent already.
    Found,
}

impl ImapSync {
    /// Sends `outgoing` (or finishes what an earlier attempt began) and
    /// files it.
    pub(super) async fn send_message(
        &self,
        account: &EmailAddress,
        settings: &ImapSettings,
        outgoing: &Outgoing,
        built: &Built,
    ) -> SyncResult<Delivery> {
        match outgoing.stage {
            SendStage::Delivered => {
                return self
                    .after_delivery(account, settings, outgoing, built, Filing::Again)
                    .await;
            }
            SendStage::DataSent => {
                let why = "an earlier attempt sent it and heard no answer";
                return self.settle(account, settings, outgoing, built, why).await;
            }
            SendStage::Queued => {}
        }
        let mut recipients: Vec<EmailAddress> = outgoing
            .draft
            .recipients()
            .map(|m| m.address.clone())
            .collect();
        recipients.sort_by_key(EmailAddress::key);
        recipients.dedup_by_key(|a| a.key());
        let transmitted = submit::without_bcc(&built.raw);
        let secret = self.secrets.get(account).await?;
        let tokens = self.tokens.as_ref();
        let mut smtp =
            SmtpConnection::open(&self.connector, account, settings, &secret, tokens).await?;
        let limit = self.connector.timeouts().command;
        let submitted =
            match submit::begin(&mut smtp, limit, account, &recipients, &transmitted).await {
                Err(stopped) => stopped,
                Ok(()) => match self.set_stage(outgoing.id, SendStage::DataSent).await {
                    Ok(()) => submit::data(&mut smtp, limit, &transmitted).await,
                    Err(e) => Submitted::Failed(e),
                },
            };
        let server = crate::imap::connect::describe(smtp.server());
        if let Err(e) = smtp.quit().await {
            tracing::debug!("SMTP: leaving {server} failed: {e}");
        }
        match submitted {
            Submitted::Accepted => {
                self.set_stage(outgoing.id, SendStage::Delivered).await?;
                self.after_delivery(account, settings, outgoing, built, Filing::New)
                    .await
            }
            Submitted::Failed(e) => {
                self.set_stage(outgoing.id, SendStage::Queued).await?;
                Err(e)
            }
            Submitted::Refused(why) => {
                self.set_stage(outgoing.id, SendStage::Queued).await?;
                Ok(Delivery::Refused(why))
            }
            Submitted::Uncertain(detail) => {
                tracing::warn!(
                    "SMTP: {server} gave no answer to a message from {account}: {detail}"
                );
                let why = format!("the connection to {server} was lost as it went ({detail})");
                self.settle(account, settings, outgoing, built, &why).await
            }
        }
    }

    /// A message whose data went without an answer: delivered if Sent
    /// has it, otherwise waiting for the person, saying `why`.
    async fn settle(
        &self,
        account: &EmailAddress,
        settings: &ImapSettings,
        outgoing: &Outgoing,
        built: &Built,
        why: &str,
    ) -> SyncResult<Delivery> {
        let waits = self.sent_waits(settings);
        let found = self
            .over(account, Lane::Outgoing, async |conn| {
                let sent = self
                    .target(conn, account, settings, SystemLabel::Sent)
                    .await?;
                match sent {
                    Some(sent) => look(conn, &sent, &built.header, waits).await,
                    None => Ok(false),
                }
            })
            .await?;
        if !found {
            return Ok(Delivery::MayHaveGone(format!(
                "It may have gone: {why}, and it is not in Sent."
            )));
        }
        self.set_stage(outgoing.id, SendStage::Delivered).await?;
        self.after_delivery(account, settings, outgoing, built, Filing::Found)
            .await
    }

    /// Files a delivered message in Sent and deletes the draft it came
    /// from. A failure here leaves it [`SendStage::Delivered`]: the next
    /// attempt does only this.
    async fn after_delivery(
        &self,
        account: &EmailAddress,
        settings: &ImapSettings,
        outgoing: &Outgoing,
        built: &Built,
        filing: Filing,
    ) -> SyncResult<Delivery> {
        self.over(account, Lane::Outgoing, async |conn| {
            if filing != Filing::Found
                && let Some(refused) = self.file(conn, account, settings, built, filing).await?
            {
                return Ok(Delivery::Refused(refused));
            }
            if let Some(old) = &outgoing.draft.replaces {
                let drafts = self.target(conn, account, settings, SystemLabel::Drafts);
                if let Some(drafts) = drafts.await? {
                    self.delete_draft(conn, account, &drafts, old).await?;
                }
            }
            Ok(Delivery::Done)
        })
        .await
    }

    /// Files `built` in Sent, unless the provider has; returns why Sent
    /// refused it, if it did.
    async fn file(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        settings: &ImapSettings,
        built: &Built,
        filing: Filing,
    ) -> SyncResult<Option<String>> {
        let Some(sent) = self
            .target(conn, account, settings, SystemLabel::Sent)
            .await?
        else {
            tracing::warn!("IMAP: {account} has no Sent folder: a sent message is not filed");
            return Ok(None);
        };
        let look_first = settings.server_files_sent || filing == Filing::Again;
        if look_first && look(conn, &sent, &built.header, self.sent_waits(settings)).await? {
            return Ok(None);
        }
        match mailbox::append(conn, &sent.name, SEEN, &built.raw).await? {
            Appended::Kept(at) => {
                self.record(account, &sent, at, &built.id, &[SEEN]).await?;
            }
            Appended::Refused(text) => {
                return Ok(Some(format!(
                    "It was sent, but {} would not keep a copy: {text}",
                    sent.name
                )));
            }
        }
        if settings.server_files_sent {
            tracing::info!(
                "IMAP: {account}'s server did not file a sent message in Sent; \
                 Postrinch files them from now on"
            );
            let mut changed = settings.clone();
            changed.server_files_sent = false;
            let a = account.clone();
            self.with_store(move |s| s.set_imap_settings(&a, &changed))
                .await?;
        }
        Ok(None)
    }
}

/// Whether `target` holds the message whose `Message-ID` is `header`:
/// looked for at once, then after each of `waits`.
async fn look(
    conn: &mut Conn,
    target: &Target,
    header: &str,
    waits: &[Duration],
) -> SyncResult<bool> {
    for wait in std::iter::once(&Duration::ZERO).chain(waits) {
        tokio::time::sleep(*wait).await;
        // Opened again each time, to see what came meanwhile.
        mailbox::examine(conn, &target.name).await?;
        if !mailbox::find(conn, header).await?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}
