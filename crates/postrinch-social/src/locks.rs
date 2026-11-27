//! Locks by account: one async mutex per account's store key, made when
//! first wanted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::OwnedMutexGuard;

/// One lock per account, shared by the engine's clones.
#[derive(Clone, Default)]
pub(crate) struct Locks {
    by_account: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl Locks {
    /// Wait for, then hold, the lock of the account whose store key is
    /// `key`, until the guard is dropped.
    pub(crate) async fn hold(&self, key: String) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.by_account.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(locks.entry(key).or_default())
        };
        lock.lock_owned().await
    }
}
