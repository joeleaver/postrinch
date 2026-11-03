//! Finding the messages whose bodies have not been downloaded yet.

use postrinch_model::{EmailAddress, ProviderMessageId};
use rhypedb_engine::CompareOp;

use crate::codec::Fields;
use crate::keys::scoped;
use crate::messages::TYPE;
use crate::{Store, StoreResult};

impl Store {
    /// The ids of an account's messages known only by their headers, newest
    /// first: the order to download them in.
    pub fn undownloaded(&self, account: &EmailAddress) -> StoreResult<Vec<ProviderMessageId>> {
        let prefix = scoped(account, "");
        let mut found: Vec<(i64, ProviderMessageId)> = Vec::new();
        for object in self
            .db
            .filter_scan_bool(TYPE, "downloaded", CompareOp::Eq, false, None)?
        {
            let fields = Fields::new(TYPE, object);
            let key = fields.str("key")?;
            if let Some(id) = key.strip_prefix(&prefix) {
                found.push((
                    fields.datetime("received")?,
                    ProviderMessageId(id.to_owned()),
                ));
            }
        }
        found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Ok(found.into_iter().map(|(_, id)| id).collect())
    }
}
