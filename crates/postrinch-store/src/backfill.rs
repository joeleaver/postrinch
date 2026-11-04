//! Finding the messages whose bodies have not been downloaded yet.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use postrinch_model::{EmailAddress, ProviderMessageId};
use rhypedb_engine::CompareOp;

use crate::codec::Fields;
use crate::keys::scoped;
use crate::messages::TYPE;
use crate::{Store, StoreResult};

/// How long a count of undownloaded messages is used.
const KEEP_COUNTS: Duration = Duration::from_secs(60);

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

    /// How many messages each account has known only by their headers, by
    /// the account's key. Counting reads every such message, so a count is
    /// kept for a minute: it says roughly how much a search cannot see,
    /// and downloading changes it all the time.
    pub fn undownloaded_counts(&self) -> StoreResult<HashMap<String, usize>> {
        let mut kept = self.undownloaded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((when, counts)) = kept.as_ref()
            && when.elapsed() < KEEP_COUNTS
        {
            return Ok(counts.clone());
        }
        let mut counts = HashMap::new();
        for object in self
            .db
            .filter_scan_bool(TYPE, "downloaded", CompareOp::Eq, false, None)?
        {
            let key = Fields::new(TYPE, object).str("key")?;
            if let Some((account, _)) = key.split_once('\u{1f}') {
                *counts.entry(account.to_owned()).or_insert(0) += 1;
            }
        }
        *kept = Some((Instant::now(), counts.clone()));
        Ok(counts)
    }
}
