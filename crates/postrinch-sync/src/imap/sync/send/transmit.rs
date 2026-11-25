//! Sending a message over SMTP (docs/imap.md, "Writing and sending"), in
//! as many transactions as the server needs.
//!
//! How far an attempt got is recorded with the message: just before a
//! transaction's data goes, [`SendStage::DataSent`] and whom it is for;
//! once the server took it, whom it went to, and [`SendStage::Delivered`]
//! when that is everyone. A transaction whose data went without an answer
//! is settled ([`ImapSync::settle`]) rather than sent again. A server
//! that takes only so many recipients at once gets the message in parts,
//! each recipient in one part only: whom a part went to is recorded
//! before the next, so no attempt sends it to them again.

use postrinch_model::{EmailAddress, ImapSettings};
use postrinch_store::{Outgoing, SendStage};

use super::drafts::Replaced;
use super::sent::{Filing, Settled, list};
use super::submit::{self, Submitted};
use super::{Built, Delivery};
use crate::imap::SmtpConnection;
use crate::imap::sync::ImapSync;
use crate::{SyncError, SyncResult};

/// How one attempt's transactions ended.
#[derive(Debug)]
enum Outcome {
    /// Everyone left has it.
    Taken,
    /// This transaction's data did not go: to try again.
    Stopped(SyncError),
    /// The server took this transaction, but recording that failed. It
    /// stays [`SendStage::DataSent`], so the next attempt settles it.
    Unrecorded(SyncError),
    /// The server refused it for good, in these words.
    Refused(String),
    /// This transaction's data went without an answer, saying why, to
    /// these recipients.
    Uncertain(String, Vec<EmailAddress>),
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
        replaced: &Replaced,
    ) -> SyncResult<Delivery> {
        let id = outgoing.id;
        let mut delivered = outgoing.delivered_to.clone();
        let mut found = false;
        match outgoing.stage {
            SendStage::Delivered => {
                return self
                    .after_delivery(account, settings, built, replaced, Filing::Again)
                    .await;
            }
            SendStage::DataSent => {
                let sending = if outgoing.sending_to.is_empty() {
                    left(outgoing, &delivered)
                } else {
                    outgoing.sending_to.clone()
                };
                let why = "an earlier attempt sent it and heard no answer";
                match self
                    .settle(account, settings, built, (&delivered, &sending), why)
                    .await?
                {
                    Settled::Went => {
                        found = true;
                        delivered.extend(sending);
                        self.delivered(id, &delivered, SendStage::Queued).await?;
                    }
                    Settled::Waits(delivery) => return Ok(delivery),
                }
            }
            SendStage::Queued => {}
        }
        loop {
            let left = left(outgoing, &delivered);
            if left.is_empty() {
                self.delivered(id, &delivered, SendStage::Delivered).await?;
                let filing = if found { Filing::Found } else { Filing::New };
                return self
                    .after_delivery(account, settings, built, replaced, filing)
                    .await;
            }
            if let Some(why) = built.incomplete() {
                return Ok(Delivery::Refused(partly(&delivered, &why)));
            }
            let outcome = self
                .transactions(account, settings, id, built, &left, &mut delivered)
                .await?;
            match outcome {
                Outcome::Taken => {}
                Outcome::Stopped(e) => {
                    self.with_store(move |s| s.outgoing_not_sent(id)).await?;
                    return Err(e);
                }
                Outcome::Unrecorded(e) => return Err(e),
                Outcome::Refused(why) => {
                    self.with_store(move |s| s.outgoing_not_sent(id)).await?;
                    return Ok(Delivery::Refused(partly(&delivered, &why)));
                }
                Outcome::Uncertain(why, sending) => {
                    let settled = self
                        .settle(account, settings, built, (&delivered, &sending), &why)
                        .await?;
                    match settled {
                        Settled::Went => {
                            found = true;
                            delivered.extend(sending);
                            self.delivered(id, &delivered, SendStage::Queued).await?;
                        }
                        Settled::Waits(delivery) => return Ok(delivery),
                    }
                }
            }
        }
    }

    /// Sends `built` to `left` over one SMTP connection, a transaction at
    /// a time while the server takes only some of them, adding whom each
    /// went to to `delivered` (and recording it).
    async fn transactions(
        &self,
        account: &EmailAddress,
        settings: &ImapSettings,
        id: u64,
        built: &Built,
        left: &[EmailAddress],
        delivered: &mut Vec<EmailAddress>,
    ) -> SyncResult<Outcome> {
        let transmitted = submit::without_bcc(&built.raw);
        let secret = self.secrets.get(account).await?;
        let tokens = self.tokens.as_ref();
        let mut smtp =
            SmtpConnection::open(&self.connector, account, settings, &secret, tokens).await?;
        let server = crate::imap::connect::describe(smtp.server());
        let limit = self.connector.timeouts().command;
        let mut rest = left;
        let outcome = loop {
            let taken = match submit::begin(&mut smtp, limit, account, rest, &transmitted).await {
                Ok(taken) => taken,
                Err(stopped) => break Outcome::ended(stopped, Vec::new(), &server),
            };
            let (to, after) = rest.split_at(taken);
            let sending = to.to_vec();
            if let Err(e) = self
                .with_store(move |s| s.outgoing_sending(id, &sending))
                .await
            {
                break Outcome::Stopped(e);
            }
            match submit::data(&mut smtp, limit, &transmitted).await {
                Submitted::Accepted => {
                    delivered.extend_from_slice(to);
                    let stage = if after.is_empty() {
                        SendStage::Delivered
                    } else {
                        SendStage::Queued
                    };
                    if let Err(e) = self.delivered(id, delivered, stage).await {
                        break Outcome::Unrecorded(e);
                    }
                    if after.is_empty() {
                        break Outcome::Taken;
                    }
                    rest = after;
                }
                other => break Outcome::ended(other, to.to_vec(), &server),
            }
        };
        if let Err(e) = smtp.quit().await {
            tracing::debug!("SMTP: leaving {server} failed: {e}");
        }
        if let Outcome::Uncertain(detail, _) = &outcome {
            tracing::warn!("SMTP: {server} gave no answer to a message from {account}: {detail}");
        }
        Ok(outcome)
    }

    /// Records whom the server took it for, and how far it got.
    async fn delivered(
        &self,
        id: u64,
        delivered: &[EmailAddress],
        stage: SendStage,
    ) -> SyncResult<()> {
        let delivered = delivered.to_vec();
        self.with_store(move |s| s.outgoing_delivered(id, &delivered, stage))
            .await
    }
}

impl Outcome {
    /// How a transaction to `to` on `server` that the server did not
    /// take ended.
    fn ended(submitted: Submitted, to: Vec<EmailAddress>, server: &str) -> Self {
        match submitted {
            Submitted::Accepted => Self::Taken,
            Submitted::Failed(e) => Self::Stopped(e),
            Submitted::Refused(why) => Self::Refused(why),
            Submitted::Uncertain(detail) => Self::Uncertain(
                format!("the connection to {server} was lost as it went ({detail})"),
                to,
            ),
        }
    }
}

/// Everyone the message is for that it has not gone to, each once, in a
/// steady order.
fn left(outgoing: &Outgoing, delivered: &[EmailAddress]) -> Vec<EmailAddress> {
    let mut recipients: Vec<EmailAddress> = outgoing
        .draft
        .recipients()
        .map(|m| m.address.clone())
        .filter(|a| !delivered.iter().any(|d| d.key() == a.key()))
        .collect();
    recipients.sort_by_key(EmailAddress::key);
    recipients.dedup_by_key(|a| a.key());
    recipients
}

/// `why` it stopped, saying whom it went to first, if anyone.
fn partly(delivered: &[EmailAddress], why: &str) -> String {
    if delivered.is_empty() {
        return why.to_owned();
    }
    format!("It went to {}, but not to the rest: {why}", list(delivered))
}
