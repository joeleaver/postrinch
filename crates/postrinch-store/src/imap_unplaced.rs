//! Copies a change made on an IMAP server without UIDPLUS whose UID the
//! server did not say and which were not found by `Message-ID`
//! ([`ImapUnplacedCopy`]; docs/imap.md, "Changing mail"). Each links to
//! its folder, cascading, so removing the folder or the account removes
//! it; one sync has read past means nothing and is removed when next met.

use postrinch_model::{EmailAddress, ImapUnplacedCopy, ProviderMessageId};
use rhypedb_engine::CompareOp;

use crate::codec::{Fields, FieldsBuilder};
use crate::imap_state::FOLDER;
use crate::keys::{SEP, scoped};
use crate::{Store, StoreError, StoreResult};

const UNPLACED: &str = "ImapUnplacedCopy";

fn key(account: &EmailAddress, folder: &str, message: &ProviderMessageId) -> String {
    format!("{}{SEP}{}", scoped(account, folder), message.0)
}

fn decode(
    folder: &str,
    message: &ProviderMessageId,
    fields: &Fields,
) -> StoreResult<ImapUnplacedCopy> {
    Ok(ImapUnplacedCopy {
        folder: folder.to_owned(),
        message: message.clone(),
        uidvalidity: fields.u32("uidvalidity")?,
        uidnext: fields.u32("uidnext")?,
        read_uidvalidity: fields.u32("read_uidvalidity")?,
    })
}

impl Store {
    /// Keep a copy made whose UID is not known. Its folder must be kept
    /// already.
    pub fn put_imap_unplaced_copy(
        &self,
        account: &EmailAddress,
        copy: &ImapUnplacedCopy,
    ) -> StoreResult<()> {
        let folder_key = scoped(account, &copy.folder);
        let folder = self.find(FOLDER, "key", &folder_key)?.ok_or_else(|| {
            StoreError::UnknownImapFolder {
                account: account.clone(),
                id: copy.folder.clone(),
            }
        })?;
        let key = key(account, &copy.folder, &copy.message);
        let fields = || {
            FieldsBuilder::new()
                .u32("uidvalidity", copy.uidvalidity)
                .u32("uidnext", copy.uidnext)
                .u32("read_uidvalidity", copy.read_uidvalidity)
        };
        let create = fields()
            .str("key", key.clone())
            .relation("folder", folder.id())
            .str("label_id", copy.folder.clone())
            .str("message", scoped(account, &copy.message.0))
            .build();
        self.upsert(UNPLACED, "key", &key, create, fields().build())?;
        Ok(())
    }

    /// The copy of `message` made in the folder that is label `folder`
    /// whose UID is not known, if one is kept.
    pub fn imap_unplaced_copy(
        &self,
        account: &EmailAddress,
        folder: &str,
        message: &ProviderMessageId,
    ) -> StoreResult<Option<ImapUnplacedCopy>> {
        self.find(UNPLACED, "key", &key(account, folder, message))?
            .map(|fields| decode(folder, message, &fields))
            .transpose()
    }

    /// Every copy of `message` kept whose UID is not known, in no
    /// particular order.
    pub fn imap_unplaced_copies(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
    ) -> StoreResult<Vec<ImapUnplacedCopy>> {
        let found = self.db.filter_scan_str(
            UNPLACED,
            "message",
            CompareOp::Eq,
            &scoped(account, &message.0),
            None,
        )?;
        found
            .into_iter()
            .map(|o| {
                let fields = Fields::new(UNPLACED, o);
                decode(&fields.str("label_id")?, message, &fields)
            })
            .collect()
    }

    /// Forget a copy whose UID was not known: sync has found it, or it
    /// means nothing now.
    pub fn remove_imap_unplaced_copy(
        &self,
        account: &EmailAddress,
        folder: &str,
        message: &ProviderMessageId,
    ) -> StoreResult<()> {
        if let Some(found) = self.find(UNPLACED, "key", &key(account, folder, message))? {
            self.db_delete(UNPLACED, found.id())?;
        }
        Ok(())
    }
}
