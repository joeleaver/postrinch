//! Copies a change asked an IMAP server to make that it has not been seen
//! to make ([`ImapCopyAttempt`]; docs/imap.md, "Keeping every message
//! safe"). Each links to its folder, cascading, so removing the folder or
//! the account removes it.

use postrinch_model::{EmailAddress, ImapCopyAttempt, ProviderMessageId};
use rhypedb_engine::CompareOp;

use crate::codec::{Fields, FieldsBuilder};
use crate::imap_state::FOLDER;
use crate::keys::{SEP, scoped};
use crate::{Store, StoreError, StoreResult};

const ATTEMPT: &str = "ImapCopyAttempt";

fn key(account: &EmailAddress, folder: &str, message: &ProviderMessageId) -> String {
    format!("{}{SEP}{}", scoped(account, folder), message.0)
}

fn decode(
    folder: &str,
    message: &ProviderMessageId,
    fields: &Fields,
) -> StoreResult<ImapCopyAttempt> {
    Ok(ImapCopyAttempt {
        folder: folder.to_owned(),
        message: message.clone(),
        uidvalidity: fields.u32("uidvalidity")?,
        uidnext: fields.u32("uidnext")?,
    })
}

impl Store {
    /// Keep an attempt to copy a message into a folder, replacing the
    /// one kept for that folder and message. Its folder must be kept
    /// already.
    pub fn put_imap_copy_attempt(
        &self,
        account: &EmailAddress,
        attempt: &ImapCopyAttempt,
    ) -> StoreResult<()> {
        let folder_key = scoped(account, &attempt.folder);
        let folder = self.find(FOLDER, "key", &folder_key)?.ok_or_else(|| {
            StoreError::UnknownImapFolder {
                account: account.clone(),
                id: attempt.folder.clone(),
            }
        })?;
        let key = key(account, &attempt.folder, &attempt.message);
        let fields = || {
            FieldsBuilder::new()
                .u32("uidvalidity", attempt.uidvalidity)
                .u32("uidnext", attempt.uidnext)
        };
        let create = fields()
            .str("key", key.clone())
            .relation("folder", folder.id())
            .str("label_id", attempt.folder.clone())
            .str("message", scoped(account, &attempt.message.0))
            .build();
        self.upsert(ATTEMPT, "key", &key, create, fields().build())?;
        Ok(())
    }

    /// Every attempt kept to copy `message`, in no particular order.
    pub fn imap_copy_attempts(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
    ) -> StoreResult<Vec<ImapCopyAttempt>> {
        let found = self.db.filter_scan_str(
            ATTEMPT,
            "message",
            CompareOp::Eq,
            &scoped(account, &message.0),
            None,
        )?;
        found
            .into_iter()
            .map(|o| {
                let fields = Fields::new(ATTEMPT, o);
                decode(&fields.str("label_id")?, message, &fields)
            })
            .collect()
    }

    /// Forget the attempt to copy `message` into the folder that is label
    /// `folder`: its copy was found, or is known not to have been made.
    pub fn remove_imap_copy_attempt(
        &self,
        account: &EmailAddress,
        folder: &str,
        message: &ProviderMessageId,
    ) -> StoreResult<()> {
        if let Some(found) = self.find(ATTEMPT, "key", &key(account, folder, message))? {
            self.db_delete(ATTEMPT, found.id())?;
        }
        Ok(())
    }
}
