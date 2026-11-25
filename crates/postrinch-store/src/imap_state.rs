//! What IMAP sync keeps per account: each folder's state ([`ImapFolder`])
//! and each copy of a message on the server ([`ImapPlace`]) (docs/imap.md,
//! "Sync" and "A message's identity").
//!
//! A folder links to its account and a place to its folder, both
//! cascading, so removing an account removes its folders and their places,
//! and removing a folder its places. A message's places are found through
//! the place's indexed `message` field.

use postrinch_model::{EmailAddress, FolderRole, ImapFolder, ImapPlace, ProviderMessageId};
use rhypedb_engine::CompareOp;
use rhypedb_engine::object::Value;

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::{SEP, scoped};
use crate::{Store, StoreError, StoreResult};

pub(crate) const FOLDER: &str = "ImapFolder";
const PLACE: &str = "ImapPlace";

fn place_key(folder_key: &str, uidvalidity: u32, uid: u32) -> String {
    format!("{folder_key}{SEP}{uidvalidity}{SEP}{uid}")
}

fn decode_folder(fields: &Fields) -> StoreResult<ImapFolder> {
    let delimiter = match fields.opt_str("delimiter")? {
        None => None,
        Some(d) => {
            let mut chars = d.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => {
                    return Err(StoreError::corrupt(
                        FOLDER,
                        fields.id(),
                        format!("delimiter {d:?} is not one character"),
                    ));
                }
            }
        }
    };
    Ok(ImapFolder {
        label_id: fields.str("label_id")?,
        name: fields.str("name")?,
        delimiter,
        role: fields.opt_json::<FolderRole>("role")?,
        uidvalidity: fields.u32("uidvalidity")?,
        uidnext: fields.u32("uidnext")?,
        highest_modseq: fields.u64("highest_modseq")?,
        subscribed: fields.bool("subscribed")?,
        mailbox_id: fields.opt_str("mailbox_id")?,
    })
}

fn decode_place(account_prefix: &str, fields: &Fields) -> StoreResult<ImapPlace> {
    let message = fields.str("message")?;
    let message = message
        .strip_prefix(account_prefix)
        .ok_or_else(|| StoreError::corrupt(PLACE, fields.id(), "message is of another account"))?
        .to_owned();
    Ok(ImapPlace {
        folder: fields.str("label_id")?,
        uidvalidity: fields.u32("uidvalidity")?,
        uid: fields.u32("uid")?,
        message: ProviderMessageId(message),
        flags: fields.json("flags")?,
        modseq: fields.u64("modseq")?,
        arrived: fields
            .opt_datetime("arrived")?
            .map(postrinch_model::Timestamp::from_millis),
    })
}

impl Store {
    /// Keep a folder's state, as sync last saw it. Its places stay.
    pub fn put_imap_folder(&self, account: &EmailAddress, folder: &ImapFolder) -> StoreResult<()> {
        let key = scoped(account, &folder.label_id);
        let fields = || {
            FieldsBuilder::new()
                .str("label_id", folder.label_id.clone())
                .str("name", folder.name.clone())
                .opt_str("delimiter", folder.delimiter.map(String::from))
                .opt_json("role", folder.role.as_ref())
                .u32("uidvalidity", folder.uidvalidity)
                .u32("uidnext", folder.uidnext)
                .u64("highest_modseq", folder.highest_modseq)
                .bool("subscribed", folder.subscribed)
                .opt_str("mailbox_id", folder.mailbox_id.clone())
        };
        let create = fields()
            .str("key", key.clone())
            .relation("account", self.account_id(account)?)
            .build();
        self.upsert(FOLDER, "key", &key, create, fields().build())?;
        Ok(())
    }

    /// The state of the folder that is label `label_id`, if kept.
    pub fn imap_folder(
        &self,
        account: &EmailAddress,
        label_id: &str,
    ) -> StoreResult<Option<ImapFolder>> {
        self.find(FOLDER, "key", &scoped(account, label_id))?
            .as_ref()
            .map(decode_folder)
            .transpose()
    }

    /// Every folder kept for the account, in the order first kept.
    pub fn imap_folders(&self, account: &EmailAddress) -> StoreResult<Vec<ImapFolder>> {
        let mut ids = self.links("Account", self.account_id(account)?, "imap_folders")?;
        ids.sort_unstable();
        self.get_many(FOLDER, &ids)?
            .iter()
            .map(decode_folder)
            .collect()
    }

    /// Forget a folder the server no longer has, with its places.
    pub fn remove_imap_folder(&self, account: &EmailAddress, label_id: &str) -> StoreResult<()> {
        if let Some(found) = self.find(FOLDER, "key", &scoped(account, label_id))? {
            // A place is deleted with its folder (ImapPlace.folder cascades).
            self.db_delete(FOLDER, found.id())?;
        }
        Ok(())
    }

    /// Keep a copy of a message: where it is, and its flags and MODSEQ as
    /// the server has them. Its folder must be kept already.
    pub fn put_imap_place(&self, account: &EmailAddress, place: &ImapPlace) -> StoreResult<()> {
        let folder_key = scoped(account, &place.folder);
        let folder = self.find(FOLDER, "key", &folder_key)?.ok_or_else(|| {
            StoreError::UnknownImapFolder {
                account: account.clone(),
                id: place.folder.clone(),
            }
        })?;
        let key = place_key(&folder_key, place.uidvalidity, place.uid);
        let fields = || {
            FieldsBuilder::new()
                .str("message", scoped(account, &place.message.0))
                .json("flags", &place.flags)
                .u64("modseq", place.modseq)
                .opt_datetime("arrived", place.arrived.map(|t| t.as_millis()))
        };
        let create = fields()
            .str("key", key.clone())
            .relation("folder", folder.id())
            .str("label_id", place.folder.clone())
            .u32("uidvalidity", place.uidvalidity)
            .u32("uid", place.uid)
            .build();
        self.upsert(PLACE, "key", &key, create, fields().build())?;
        Ok(())
    }

    /// Forget a copy the server no longer has.
    pub fn remove_imap_place(
        &self,
        account: &EmailAddress,
        folder: &str,
        uidvalidity: u32,
        uid: u32,
    ) -> StoreResult<()> {
        let key = place_key(&scoped(account, folder), uidvalidity, uid);
        if let Some(found) = self.find(PLACE, "key", &key)? {
            self.db_delete(PLACE, found.id())?;
        }
        Ok(())
    }

    /// Every kept copy of a message, in no particular order.
    pub fn imap_message_places(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
    ) -> StoreResult<Vec<ImapPlace>> {
        let prefix = scoped(account, "");
        let found = self.db.filter_scan_str(
            PLACE,
            "message",
            CompareOp::Eq,
            &scoped(account, &message.0),
            None,
        )?;
        found
            .into_iter()
            .map(|o| decode_place(&prefix, &Fields::new(PLACE, o)))
            .collect()
    }

    /// Every kept copy in a folder, by UID.
    pub fn imap_folder_places(
        &self,
        account: &EmailAddress,
        folder: &str,
    ) -> StoreResult<Vec<ImapPlace>> {
        let Some(ids) = self.place_ids(account, folder)? else {
            return Ok(Vec::new());
        };
        let prefix = scoped(account, "");
        let mut places = self
            .get_many(PLACE, &ids)?
            .iter()
            .map(|f| decode_place(&prefix, f))
            .collect::<StoreResult<Vec<_>>>()?;
        places.sort_by_key(|p| (p.uidvalidity, p.uid));
        Ok(places)
    }

    /// The highest UID kept in a folder; `None` when it has no places.
    pub fn imap_highest_uid(
        &self,
        account: &EmailAddress,
        folder: &str,
    ) -> StoreResult<Option<u32>> {
        let Some(ids) = self.place_ids(account, folder)? else {
            return Ok(None);
        };
        let mut highest = None;
        for place in self.get_many_projected(PLACE, &ids, &["uid"])? {
            highest = highest.max(Some(place.u32("uid")?));
        }
        Ok(highest)
    }

    /// Forget every copy in a folder, as when its UIDVALIDITY changed.
    /// Returns how many went. The folder's own state stays.
    pub fn clear_imap_folder(&self, account: &EmailAddress, folder: &str) -> StoreResult<usize> {
        let Some(ids) = self.place_ids(account, folder)? else {
            return Ok(0);
        };
        for &id in &ids {
            self.db_delete(PLACE, id)?;
        }
        Ok(ids.len())
    }

    /// Forget where sync got to in each of the account's folders and every
    /// copy it knew, keeping the folders and their label ids, so the next
    /// sync reads every folder from the start ([`Store::clear_mail`]).
    ///
    /// Each folder's position goes before its copies: if this is
    /// interrupted between the two, the folder is left with a UIDVALIDITY
    /// of 0, which no server sends, so the next sync takes it as changed
    /// and forgets the copies left behind. The other order could leave a
    /// position past copies that are gone, and sync would never fetch them
    /// again.
    pub(crate) fn reset_imap_folders(&self, account: &EmailAddress) -> StoreResult<()> {
        for folder in self.imap_folders(account)? {
            let label_id = folder.label_id.clone();
            self.put_imap_folder(
                account,
                &ImapFolder {
                    uidvalidity: 0,
                    uidnext: 0,
                    highest_modseq: 0,
                    ..folder
                },
            )?;
            self.clear_imap_folder(account, &label_id)?;
        }
        Ok(())
    }

    /// The ids of a folder's places; `None` when the folder is not kept.
    fn place_ids(&self, account: &EmailAddress, folder: &str) -> StoreResult<Option<Vec<u64>>> {
        let key = Value::String(scoped(account, folder));
        match self.db.find_by_unique(FOLDER, "key", &key)? {
            Some(found) => Ok(Some(self.links(FOLDER, found.id, "places")?)),
            None => Ok(None),
        }
    }
}
