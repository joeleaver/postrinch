//! Locks by key: one async mutex per account's store key, made when
//! first wanted; and locks for keys without end (each piece of media
//! fetched), forgotten once no one holds or waits for them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::OwnedMutexGuard;

type ByKey = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

/// One lock per account, shared by the engine's clones.
#[derive(Clone, Default)]
pub(crate) struct Locks {
    by_account: ByKey,
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

/// One lock per key while it is wanted, shared by the engine's clones: a
/// key no one holds or waits for is forgotten, so keys without end do not
/// pile up.
#[derive(Clone, Default)]
pub(crate) struct PassingLocks {
    by_key: ByKey,
}

/// A held [`PassingLocks`] lock, released when dropped.
pub(crate) struct PassingGuard {
    key: String,
    by_key: ByKey,
    guard: Option<OwnedMutexGuard<()>>,
}

impl PassingLocks {
    /// Wait for, then hold, the lock of `key`, until the guard is dropped.
    pub(crate) async fn hold(&self, key: String) -> PassingGuard {
        let lock = Arc::clone(locked(&self.by_key).entry(key.clone()).or_default());
        let guard = lock.lock_owned().await;
        PassingGuard {
            key,
            by_key: Arc::clone(&self.by_key),
            guard: Some(guard),
        }
    }

    /// How many keys are remembered now.
    #[cfg(test)]
    fn len(&self) -> usize {
        locked(&self.by_key).len()
    }
}

impl Drop for PassingGuard {
    fn drop(&mut self) {
        self.guard.take();
        // Each waiter holds the lock's Arc, taken under the map's lock, so
        // a count of one (the map's own) means no one else wants it.
        let mut by_key = locked(&self.by_key);
        if by_key
            .get(&self.key)
            .is_some_and(|lock| Arc::strong_count(lock) == 1)
        {
            by_key.remove(&self.key);
        }
    }
}

fn locked(by_key: &ByKey) -> MutexGuard<'_, HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    by_key.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_passing_lock_is_forgotten_once_no_one_wants_it() {
        let locks = PassingLocks::default();
        let first = locks.hold("a".into()).await;
        let waiting = tokio::spawn({
            let locks = locks.clone();
            async move { drop(locks.hold("a".into()).await) }
        });
        tokio::task::yield_now().await;
        drop(first);
        // The waiter still wants it, then takes and releases it.
        waiting.await.unwrap();
        assert_eq!(locks.len(), 0);
        let _held = locks.hold("b".into()).await;
        assert_eq!(locks.len(), 1);
    }
}
