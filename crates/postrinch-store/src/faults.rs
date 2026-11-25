//! Write failures a test can program (feature `faults`, for the tests of
//! crates that use the store): the next so many updates of a type fail,
//! as a full or failing disk would fail them. Never built into the app.

use std::sync::Mutex;

use crate::{Store, StoreError, StoreResult};

/// The failures waiting, by type: how many updates of it fail next.
#[derive(Debug, Default)]
pub(crate) struct Faults {
    pending: Mutex<Vec<(String, usize)>>,
}

impl Faults {
    /// Fails this update of `type_name` if one is programmed.
    pub(crate) fn check(&self, type_name: &str) -> StoreResult<()> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let Some(at) = pending.iter().position(|(t, _)| t == type_name) else {
            return Ok(());
        };
        pending[at].1 -= 1;
        if pending[at].1 == 0 {
            pending.remove(at);
        }
        Err(StoreError::Io(std::io::Error::other(format!(
            "a programmed failure to write {type_name}"
        ))))
    }
}

impl Store {
    /// Makes the next `times` updates of objects of `type_name` (as
    /// `"Outgoing"`) fail, without writing anything.
    pub fn fail_updates(&self, type_name: &str, times: usize) {
        if times == 0 {
            return;
        }
        let mut pending = self
            .faults
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.retain(|(t, _)| t != type_name);
        pending.push((type_name.to_owned(), times));
    }
}
