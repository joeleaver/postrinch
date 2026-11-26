//! Drafts live in the Drafts folder (docs/imap.md, "Writing and
//! sending"): keeping one APPENDs it (`\Draft \Seen`), keeping it again
//! appends the new one and deletes the one it replaces, and sending it
//! deletes it.
//!
//! What a draft replaces is the copy actually on the server. A Keep the
//! server refused never got there, so a later Keep or Send that replaces
//! it deletes what that Keep would have replaced, and so on back
//! ([`Replaced`]), and the refused Keep, now out of date, is done with.

use postrinch_model::{EmailAddress, ImapSettings, ProviderMessageId, SystemLabel};
use postrinch_store::OutgoingKind;

use super::super::places::SEEN;
use super::super::wire::Conn;
use super::mailbox::{self, Appended, Target};
use super::{Built, Delivery};
use crate::SyncResult;
use crate::compose::outgoing_message_id;
use crate::imap::message_id;
use crate::imap::sync::ImapSync;

const DRAFT: &str = "\\Draft";

/// The drafts a draft replaces on the server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Replaced {
    /// Each draft that may be there, with its `Message-ID` when it is a
    /// refused Keep's (which sync may never have seen).
    drafts: Vec<(ProviderMessageId, Option<String>)>,
    /// The refused Keeps passed on the way, by store id: superseded once
    /// the drafts are deleted.
    superseded: Vec<u64>,
}

impl Replaced {
    /// Whether it replaces nothing.
    pub(super) fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }
}

impl ImapSync {
    /// What `replaces` stands for on the server: the draft itself and,
    /// while it names a Keep that waits for the person (refused, so never
    /// kept), the draft that Keep replaces in turn.
    pub(super) async fn replaced(
        &self,
        account: &EmailAddress,
        replaces: Option<&ProviderMessageId>,
    ) -> SyncResult<Replaced> {
        let Some(first) = replaces.cloned() else {
            return Ok(Replaced::default());
        };
        let owner = account.clone();
        let refused: Vec<_> = self
            .with_store(move |s| s.outgoing_for(&owner))
            .await?
            .into_iter()
            .filter(|o| o.kind == OutgoingKind::Keep && o.refused)
            .map(|o| {
                let header = outgoing_message_id(&o);
                (message_id(account, &header), header, o)
            })
            .collect();
        let mut replaced = Replaced::default();
        let mut next = Some(first);
        while let Some(id) = next.take() {
            if replaced.drafts.iter().any(|(seen, _)| *seen == id) {
                break;
            }
            match refused.iter().find(|(keep, _, _)| *keep == id) {
                Some((_, header, keep)) => {
                    replaced.drafts.push((id, Some(header.clone())));
                    replaced.superseded.push(keep.id);
                    next = keep.draft.replaces.clone();
                }
                None => replaced.drafts.push((id, None)),
            }
        }
        Ok(replaced)
    }

    /// Keeps `built` in Drafts, in place of the drafts `replaced` names.
    /// A draft an earlier attempt appended is found there, by its
    /// `Message-ID`, rather than appended again.
    pub(super) async fn keep_draft(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        settings: &ImapSettings,
        built: &Built,
        replaced: &Replaced,
    ) -> SyncResult<Delivery> {
        let target = self
            .target(conn, account, settings, SystemLabel::Drafts)
            .await?;
        let Some(drafts) = target else {
            return Ok(Delivery::Refused(
                "the account has no Drafts folder to keep it in".into(),
            ));
        };
        let kept = match mailbox::examine(conn, &drafts.name).await? {
            Some(_) => !mailbox::find(conn, &built.header).await?.is_empty(),
            None => false,
        };
        if !kept {
            let flags = format!("{DRAFT} {SEEN}");
            match mailbox::append(conn, &drafts.name, &flags, &built.raw).await? {
                Appended::Kept(at) => {
                    self.record(account, &drafts, at, &built.id, &[DRAFT, SEEN])
                        .await?;
                }
                Appended::Refused(text) => {
                    return Ok(Delivery::Refused(format!(
                        "{} would not keep the draft: {text}",
                        drafts.name
                    )));
                }
            }
        }
        self.delete_replaced(conn, account, &drafts, built, replaced)
            .await?;
        self.retire_superseded(replaced).await?;
        Ok(Delivery::Done)
    }

    /// Forgets the refused Keeps `replaced` passed on the way: the draft
    /// that replaced them was kept or sent, so they are out of date,
    /// whether or not there was a Drafts folder to delete drafts from.
    pub(super) async fn retire_superseded(&self, replaced: &Replaced) -> SyncResult<()> {
        let superseded = replaced.superseded.clone();
        self.with_store(move |s| superseded.iter().try_for_each(|id| s.outgoing_done(*id)))
            .await
    }

    /// Deletes the drafts `replaced` names (never `built` itself).
    pub(super) async fn delete_replaced(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        drafts: &Target,
        built: &Built,
        replaced: &Replaced,
    ) -> SyncResult<()> {
        for (old, header) in &replaced.drafts {
            if *old != built.id {
                self.delete_draft(conn, account, drafts, old, header.as_deref())
                    .await?;
            }
        }
        Ok(())
    }

    /// Deletes the copies in Drafts of the draft `old`: those sync knows
    /// of, and any its `Message-ID` (`header`, or as sync stored it) finds
    /// there. Nothing to delete (it went already) is not an error.
    async fn delete_draft(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        drafts: &Target,
        old: &ProviderMessageId,
        header: Option<&str>,
    ) -> SyncResult<()> {
        let selected = mailbox::select(conn, &drafts.name).await?;
        let uidvalidity = selected.uidvalidity;
        let (a, o, label) = (account.clone(), old.clone(), drafts.label.clone());
        let (mut uids, stored) = self
            .blocking(move |s| {
                let known: Vec<u32> = match &label {
                    Some(label) => s
                        .imap_message_places(&a, &o)?
                        .into_iter()
                        .filter(|p| &p.folder == label && p.uidvalidity == uidvalidity)
                        .map(|p| p.uid)
                        .collect(),
                    None => Vec::new(),
                };
                let header = s.message(&a, &o)?.and_then(|m| m.headers.message_id);
                Ok((known, header))
            })
            .await?;
        if let Some(header) = header.map(str::to_owned).or(stored) {
            uids.extend(mailbox::find(conn, &header).await?);
        }
        uids.sort_unstable();
        uids.dedup();
        if uids.is_empty() {
            return Ok(());
        }
        let deleted = self
            .delete_copies(conn, account, (drafts.label.clone(), uidvalidity), &uids)
            .await?;
        if let Some(label) = drafts.label.clone() {
            self.forget(account, (label, uidvalidity), uids, old, deleted)
                .await?;
        }
        Ok(())
    }
}
