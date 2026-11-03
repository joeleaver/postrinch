//! Running sync in the background and reporting it to the window.

use std::sync::Arc;
use std::thread;

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
}

/// Start keeping every account in sync on a thread of its own. `status`
/// follows what each account's sync is doing; `revision` is bumped when the
/// store changed. Fails, saying why, when sync cannot run at all.
pub fn start(
    store: Store,
    status: Signal<Vec<AccountSync>>,
    revision: Signal<u64>,
) -> Result<SyncHandle, String> {
    let path = ClientConfig::default_path()
        .ok_or_else(|| "this system has no configuration directory".to_owned())?;
    let config = ClientConfig::load(&path)
        .map_err(|e| format!("the Google OAuth client ({}): {e}", path.display()))?;
    let auth = Arc::new(GoogleAuth::new(config, Arc::new(Keychain)));
    let sync = GmailSync::new(Arc::new(GmailApi::new(auth.clone())), store.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start its runtime: {e}"))?;
    let events = Arc::new(move |event: SyncEvent| report(event, status, revision));
    let wake = Arc::new(Notify::new());
    let handle = SyncHandle {
        auth,
        store,
        runtime: runtime.handle().clone(),
        wake: wake.clone(),
    };
    thread::Builder::new()
        .name("sync".into())
        .spawn(move || runtime.block_on(sync.keep_in_sync(KeepOptions::default(), events, wake)))
        .map_err(|e| format!("cannot start its thread: {e}"))?;
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
            let (line, progress) = describe(progress);
            (account, line, progress, false, false)
        }
        SyncEvent::Synced { account, .. } => {
            (account, "New mail up to date".to_owned(), None, false, true)
        }
        SyncEvent::Done { account, .. } => (account, "Up to date".to_owned(), None, false, true),
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
