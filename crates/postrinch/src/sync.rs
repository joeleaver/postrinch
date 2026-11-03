//! Running sync in the background and reporting it to the window.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use postrinch_model::EmailAddress;
use postrinch_store::Store;
use postrinch_sync::google::{
    ClientConfig, GmailApi, GmailSync, GoogleAuth, KeepOptions, Keychain, Phase, Progress,
    SyncEvent,
};
use rinch::prelude::*;
use tokio::runtime::Handle;
use tokio::sync::Notify;

/// One account's sync, as the sidebar shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSync {
    pub account: EmailAddress,
    /// What it is doing, or how the last pass ended.
    pub line: String,
    /// `done` of `total`, while downloading.
    pub progress: Option<(usize, usize)>,
    pub trouble: bool,
}

/// What the window needs to act on accounts while sync runs.
#[derive(Clone)]
pub struct SyncHandle {
    pub auth: Arc<GoogleAuth>,
    pub store: Store,
    /// The runtime sync runs on, for sign-ins and removals.
    pub runtime: Handle,
    /// Starts sync's next pass at once.
    pub wake: Arc<Notify>,
    /// Sends the changes made here at once.
    pub send: Arc<Notify>,
    sync: GmailSync,
}

impl SyncHandle {
    /// The sync itself, for downloads the person asks for.
    pub fn sync_engine(&self) -> &GmailSync {
        &self.sync
    }
}

/// Why sync is not running: a sentence for the sidebar, and the detail
/// for the log.
#[derive(Debug)]
pub struct SyncOff {
    pub said: &'static str,
    pub detail: String,
}

impl SyncOff {
    fn new(said: &'static str, detail: impl Into<String>) -> Self {
        Self {
            said,
            detail: detail.into(),
        }
    }
}

/// Start keeping every account in sync, and sending the changes made here,
/// on a thread of its own. `status` follows what each account's sync is
/// doing; `revision` is bumped when the store changed. Fails, saying why,
/// when sync cannot run at all.
pub fn start(
    store: Store,
    status: Signal<Vec<AccountSync>>,
    revision: Signal<u64>,
) -> Result<SyncHandle, SyncOff> {
    let Some(path) = ClientConfig::default_path() else {
        return Err(SyncOff::new(
            "Sync is off: this computer has no settings folder.",
            "no configuration directory",
        ));
    };
    let config = ClientConfig::load(&path).map_err(|e| {
        let said = if path.exists() {
            "Sync is off: the Google sign-in settings could not be read."
        } else {
            "Sync is off: Google sign-in is not set up on this computer."
        };
        SyncOff::new(said, format!("{}: {e}", path.display()))
    })?;
    let auth = Arc::new(GoogleAuth::new(config, Arc::new(Keychain)));
    let sync = GmailSync::new(Arc::new(GmailApi::new(auth.clone())), store.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            SyncOff::new(
                "Sync is off: it could not start.",
                format!("its runtime: {e}"),
            )
        })?;
    let events = Arc::new(move |event: SyncEvent| report(event, status, revision));
    let (wake, send) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let handle = SyncHandle {
        auth,
        store,
        runtime: runtime.handle().clone(),
        wake: wake.clone(),
        send: send.clone(),
        sync: sync.clone(),
    };
    let options = KeepOptions::default();
    thread::Builder::new()
        .name("sync".into())
        .spawn(move || {
            runtime.block_on(async {
                tokio::join!(
                    sync.keep_in_sync(options, events.clone(), wake),
                    sync.keep_sending(options, events, send),
                )
            })
        })
        .map_err(|e| {
            SyncOff::new(
                "Sync is off: it could not start.",
                format!("its thread: {e}"),
            )
        })?;
    Ok(handle)
}

/// Turn a sync event into the sidebar's line for the account, from the
/// sync thread.
fn report(event: SyncEvent, status: Signal<Vec<AccountSync>>, revision: Signal<u64>) {
    let (account, line, progress, trouble, changed) = match event {
        SyncEvent::Started { account } => (
            account,
            "Checking for new mail".to_owned(),
            None,
            false,
            false,
        ),
        SyncEvent::Progress { account, progress } => {
            // Mail arriving shows as it arrives, not when the pass ends,
            // which for a new account's first sync is hours away.
            let changed = adds_mail(progress) && refresh_due();
            let (line, progress) = describe(progress);
            (account, line, progress, false, changed)
        }
        // A pass that found nothing new changes nothing the window shows.
        SyncEvent::Synced { account, report } => {
            let changed = report.headers + report.fetched + report.relabelled + report.removed > 0;
            (
                account,
                "New mail up to date".to_owned(),
                None,
                false,
                changed,
            )
        }
        SyncEvent::Done {
            account,
            downloaded,
        } => (
            account,
            "Up to date".to_owned(),
            None,
            false,
            downloaded > 0,
        ),
        SyncEvent::Failed {
            account,
            error,
            retry_in,
        } => {
            tracing::warn!("{account}: {error}");
            let line = format!(
                "Could not sync; trying again in {} s",
                retry_in.as_secs().max(1)
            );
            (account, line, None, true, false)
        }
        SyncEvent::NeedsSignIn { account } => {
            (account, "Needs to sign in".to_owned(), None, true, true)
        }
        // Changes are shown as made; sending them is news only when it
        // goes wrong.
        SyncEvent::Sent { report, .. } if report.refused.is_empty() => return,
        SyncEvent::Sent { account, report } => {
            tracing::warn!("{account}: Gmail refused changes: {:?}", report.refused);
            let line = match report.refused.len() {
                1 => "Gmail refused a change; showing Gmail's mail".to_owned(),
                n => format!("Gmail refused {n} changes; showing Gmail's mail"),
            };
            (account, line, None, true, true)
        }
        SyncEvent::SendFailed {
            account,
            error,
            retry_in,
        } => {
            tracing::warn!("{account}: sending changes: {error}");
            let line = format!(
                "Could not send changes; trying again in {} s",
                retry_in.as_secs().max(1)
            );
            (account, line, None, true, false)
        }
    };
    status.update_send(move |all| {
        let entry = AccountSync {
            account: account.clone(),
            line,
            progress,
            trouble,
        };
        match all.iter_mut().find(|s| s.account == account) {
            Some(existing) => *existing = entry,
            None => all.push(entry),
        }
    });
    if changed {
        revision.update_send(|n| *n += 1);
    }
}

/// Whether sync is storing mail the lists should show.
fn adds_mail(progress: Progress) -> bool {
    matches!(
        progress,
        Progress::Working {
            phase: Phase::Headers | Phase::Changes,
            ..
        }
    )
}

/// How often the window reloads while sync adds mail. A reload of a big
/// folder takes seconds, so more often keeps the loader busy for nothing.
const REFRESH_EVERY: Duration = Duration::from_secs(15);

/// Whether it has been [`REFRESH_EVERY`] since the last reload this asked
/// for; if so, it counts as asked for now.
fn refresh_due() -> bool {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let due = last.is_none_or(|t| t.elapsed() >= REFRESH_EVERY);
    if due {
        *last = Some(Instant::now());
    }
    due
}

fn describe(progress: Progress) -> (String, Option<(usize, usize)>) {
    match progress {
        Progress::Listing { found } => (format!("Listing mail: {}", thousands(found)), None),
        Progress::Working { phase, done, total } => {
            let what = match phase {
                Phase::Headers => "Fetching new mail",
                Phase::Changes => "Applying changes",
                Phase::Bodies => "Downloading older mail",
            };
            (
                format!("{what}: {} of {}", thousands(done), thousands(total)),
                Some((done, total)),
            )
        }
    }
}

/// 3718 as "3,718".
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::thousands;

    #[test]
    fn groups_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(3718), "3,718");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
