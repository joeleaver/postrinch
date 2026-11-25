//! Drafts live in the Drafts folder (docs/imap.md, "Writing and
//! sending"): keeping one APPENDs it (`\Draft \Seen`), keeping it again
//! appends the new one and deletes the one it replaces, and sending it
//! deletes it.

use postrinch_model::{EmailAddress, ImapSettings, ProviderMessageId, SystemLabel};

use super::super::places::SEEN;
use super::super::wire::Conn;
use super::mailbox::{self, Appended, Target};
use super::{Built, Delivery};
use crate::SyncResult;
use crate::imap::sync::ImapSync;

const DRAFT: &str = "\\Draft";

impl ImapSync {
    /// Keeps `built` in Drafts, in place of the draft `replaces` names.
    /// A draft an earlier attempt appended is found there, by its
    /// `Message-ID`, rather than appended again.
    pub(super) async fn keep_draft(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        settings: &ImapSettings,
        built: &Built,
        replaces: Option<&ProviderMessageId>,
    ) -> SyncResult<Delivery> {
        let target = self
            .target(conn, account, settings, SystemLabel::Drafts)
            .await?;
        let Some(drafts) = target else {
            return Ok(Delivery::Refused(
                "the account has no Drafts folder to keep it in".into(),
            ));
        };
        mailbox::examine(conn, &drafts.name).await?;
        if mailbox::find(conn, &built.header).await?.is_empty() {
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
        if let Some(old) = replaces.filter(|old| **old != built.id) {
            self.delete_draft(conn, account, &drafts, old).await?;
        }
        Ok(Delivery::Done)
    }

    /// Deletes the copies in Drafts of the draft `old`: those sync knows
    /// of, and any its `Message-ID` finds there. Nothing to delete (it
    /// went already) is not an error.
    pub(super) async fn delete_draft(
        &self,
        conn: &mut Conn,
        account: &EmailAddress,
        drafts: &Target,
        old: &ProviderMessageId,
    ) -> SyncResult<()> {
        let selected = mailbox::select(conn, &drafts.name).await?;
        let uidvalidity = selected.uidvalidity;
        let (a, o, label) = (account.clone(), old.clone(), drafts.label.clone());
        let (mut uids, header) = self
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
        if let Some(header) = header {
            uids.extend(mailbox::find(conn, &header).await?);
        }
        uids.sort_unstable();
        uids.dedup();
        if uids.is_empty() {
            return Ok(());
        }
        let deleted = mailbox::delete(conn, &uids).await?;
        if let Some(label) = drafts.label.clone() {
            self.forget(account, (label, uidvalidity), uids, old, deleted)
                .await?;
        }
        Ok(())
    }
}
