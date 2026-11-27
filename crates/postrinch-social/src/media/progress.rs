//! How far each upload has got, for the window to show: told live
//! ([`LiveEvent::Uploading`]) as each whole percent goes, and kept in
//! memory while it runs ([`Social::uploads`]), so a window opened in the
//! middle of one sees it too.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use postrinch_ofscp::http::{Progress, ProgressFn};
use postrinch_store::social_account_key;

use crate::live::{LiveEvent, Registry};
use crate::{Social, SocialResult, parse_actor};

/// One file of a queued change on its way to the home provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadProgress {
    /// The account's actor.
    pub account: String,
    /// The queued change the file is for.
    pub client_id: String,
    /// The place the change goes to.
    pub place: Option<String>,
    /// Which of the change's files still to upload this is, from 0.
    pub file: usize,
    /// How many of the change's files were still to upload.
    pub files: usize,
    /// The file's name.
    pub name: String,
    /// Bytes of the upload sent so far. A retry starts again from 0.
    pub sent: u64,
    /// The whole upload's length: the file, and the form around it.
    pub total: u64,
}

/// No percent told yet.
const NONE: u64 = u64::MAX;

/// The uploads running now, by account key and client id.
#[derive(Default)]
pub(crate) struct Uploads {
    running: Mutex<HashMap<(String, String), UploadProgress>>,
}

impl Uploads {
    fn running(&self) -> MutexGuard<'_, HashMap<(String, String), UploadProgress>> {
        self.running.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Watches one file's upload while it lives: keeps and tells its progress,
/// and forgets it when dropped, however the upload ended.
pub(crate) struct Watching {
    uploads: Arc<Uploads>,
    key: (String, String),
    callback: ProgressFn,
}

impl Watching {
    /// Watch the upload `start` describes (its `sent` and `total` are 0).
    pub(crate) fn start(
        uploads: &Arc<Uploads>,
        live: &Arc<Registry>,
        start: UploadProgress,
    ) -> Self {
        let key = (social_account_key(&start.account), start.client_id.clone());
        uploads.running().insert(key.clone(), start.clone());
        // The last whole percent told; NONE before any is. Told again each
        // time it changes, so at most about a hundred times a file (and
        // again from 0 when the transport retries).
        let told = Arc::new(AtomicU64::new(NONE));
        let callback: ProgressFn = {
            let (uploads, live, key) = (Arc::clone(uploads), Arc::clone(live), key.clone());
            Arc::new(move |progress: Progress| {
                let percent = (progress.sent.saturating_mul(100))
                    .checked_div(progress.total)
                    .unwrap_or(100);
                let mut running = uploads.running();
                let Some(now) = running.get_mut(&key) else {
                    return;
                };
                now.sent = progress.sent;
                now.total = progress.total;
                if told.swap(percent, Ordering::Relaxed) != percent {
                    live.tell(LiveEvent::Uploading(now.clone()));
                }
            })
        };
        Self {
            uploads: Arc::clone(uploads),
            key,
            callback,
        }
    }

    /// What the transport tells of the upload's progress.
    pub(crate) fn callback(&self) -> ProgressFn {
        Arc::clone(&self.callback)
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.uploads.running().remove(&self.key);
    }
}

impl Social {
    /// The uploads `actor`'s outbox is making now, as far as each has got.
    pub fn uploads(&self, actor: &str) -> SocialResult<Vec<UploadProgress>> {
        let key = social_account_key(&parse_actor(actor)?.to_string());
        let mut found: Vec<UploadProgress> = self
            .media
            .uploads
            .running()
            .iter()
            .filter(|((account, _), _)| *account == key)
            .map(|(_, progress)| progress.clone())
            .collect();
        found.sort_by(|a, b| a.client_id.cmp(&b.client_id));
        Ok(found)
    }
}
