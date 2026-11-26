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
//! before the next, so no attempt sends it to them again. A recipient the
//! server puts off for now goes on a later attempt ([`super::put_off`]).
//! Recording that the server took a part is tried again before giving
//! up; if it still fails, the message waits for the person, warned that
//! sending it again would send it twice.

use std::time::Duration;

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
    /// The server took this transaction, for these recipients, but
    /// recording that failed, tried again too. It stays
    /// [`SendStage::DataSent`]: never sent again without the person.
    Unrecorded(SyncError, Vec<EmailAddress>),
    /// The server refused it for good, in these words.
    Refused(String),
    /// This transaction's data went without an answer, saying why, to
    /// these recipients.
    Uncertain(String, Vec<EmailAddress>),
    /// Everyone left has it but these, whom the server (described) put
    /// off for now, in its words.
    PutOff(String, Vec<(EmailAddress, String)>),
}

/// The pauses before recording that the server took a transaction is
/// tried again: a failing disk may recover in a moment.
const RECORD_PAUSES: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(500)];

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
                        self.record_taken(id, &delivered, SendStage::Queued).await?;
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
                Outcome::Unrecorded(e, to) => {
                    tracing::warn!(
                        "IMAP: {account}: the server took a message, but recording that \
                         failed: {e}"
                    );
                    let rest = self::left(outgoing, &delivered);
                    return Ok(Delivery::MayHaveGone(unrecorded(
                        &delivered, &to, &rest, &e,
                    )));
                }
                Outcome::PutOff(server, put_off) => {
                    return self.put_off(id, &server, put_off, &delivered).await;
                }
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
                            self.record_taken(id, &delivered, SendStage::Queued).await?;
                        }
                        Settled::Waits(delivery) => return Ok(delivery),
                    }
                }
            }
        }
    }

    /// Sends `built` to `left` over one SMTP connection, a transaction at
    /// a time while the server takes only some of them, adding whom each
    /// went to to `delivered` (and recording it). Those the server puts
    /// off are left out, and returned ([`Outcome::PutOff`]) once the rest
    /// have it.
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
        let mut rest = left.to_vec();
        let mut put_off = Vec::new();
        let outcome = loop {
            let envelope = match submit::begin(&mut smtp, limit, account, &rest, &transmitted).await
            {
                Ok(envelope) => envelope,
                Err(stopped) => break Outcome::ended(stopped, Vec::new(), &server),
            };
            put_off.extend(envelope.put_off);
            rest = envelope.rest;
            // None taken: every one left was put off (none taken and "too
            // many" is an error).
            if envelope.taken.is_empty() {
                break Outcome::Taken;
            }
            let to = envelope.taken;
            let sending = to.clone();
            if let Err(e) = self
                .with_store(move |s| s.outgoing_sending(id, &sending))
                .await
            {
                break Outcome::Stopped(e);
            }
            match submit::data(&mut smtp, limit, &transmitted).await {
                Submitted::Accepted => {
                    delivered.extend_from_slice(&to);
                    let stage = if rest.is_empty() && put_off.is_empty() {
                        SendStage::Delivered
                    } else {
                        SendStage::Queued
                    };
                    if let Err(e) = self.record_taken(id, delivered, stage).await {
                        break Outcome::Unrecorded(e, to);
                    }
                    if rest.is_empty() {
                        break Outcome::Taken;
                    }
                }
                other => break Outcome::ended(other, to, &server),
            }
        };
        let outcome = match outcome {
            Outcome::Taken if !put_off.is_empty() => Outcome::PutOff(server.clone(), put_off),
            other => other,
        };
        if let Err(e) = smtp.quit().await {
            tracing::debug!("SMTP: leaving {server} failed: {e}");
        }
        if let Outcome::Uncertain(detail, _) = &outcome {
            tracing::warn!("SMTP: {server} gave no answer to a message from {account}: {detail}");
        }
        Ok(outcome)
    }

    /// Records whom the server took it for, and how far it got, trying
    /// again after each of [`RECORD_PAUSES`]: what the server took must
    /// not be forgotten, or it would be sent again.
    async fn record_taken(
        &self,
        id: u64,
        delivered: &[EmailAddress],
        stage: SendStage,
    ) -> SyncResult<()> {
        let mut pauses = RECORD_PAUSES.iter();
        loop {
            match self.delivered(id, delivered, stage).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let Some(pause) = pauses.next() else {
                        return Err(e);
                    };
                    tracing::warn!("IMAP: recording a delivered message failed, again soon: {e}");
                    tokio::time::sleep(*pause).await;
                }
            }
        }
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

/// What the person is told when the server took a transaction to `to`
/// but recording that failed (`error`): it went to `delivered` (`to`
/// among them), sending it again would send it to `to` twice, and it has
/// not gone to `left`.
fn unrecorded(
    delivered: &[EmailAddress],
    to: &[EmailAddress],
    left: &[EmailAddress],
    error: &SyncError,
) -> String {
    let mut why = format!(
        "It went to {}, but Postrinch could not record that ({error}): sending it again \
         would send it to {} a second time.",
        list(delivered),
        list(to)
    );
    if !left.is_empty() {
        why.push_str(&format!(" It has not gone to {}.", list(left)));
    }
    why
}

/// `why` it stopped, saying whom it went to first, if anyone.
pub(super) fn partly(delivered: &[EmailAddress], why: &str) -> String {
    if delivered.is_empty() {
        return why.to_owned();
    }
    format!("It went to {}, but not to the rest: {why}", list(delivered))
}
