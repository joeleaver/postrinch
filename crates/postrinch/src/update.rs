//! Keeping the installed window current (`postrinch-update`): a check a
//! minute after start and daily after, "Restart to update" in the title bar
//! once a new version is staged, and Settings › About saying where it
//! stands. A window outside an install does none of it.

use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use postrinch_update::{Icons, check_and_stage, managed_dir, staged_version};
use rinch::prelude::*;

use crate::state::State;

const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The icon `--install` puts in the desktop's theme.
pub const ICONS: Icons = Icons {
    pngs: &[
        (16, include_bytes!("../assets/icons/postrinch-16.png")),
        (24, include_bytes!("../assets/icons/postrinch-24.png")),
        (32, include_bytes!("../assets/icons/postrinch-32.png")),
        (48, include_bytes!("../assets/icons/postrinch-48.png")),
        (64, include_bytes!("../assets/icons/postrinch-64.png")),
        (128, include_bytes!("../assets/icons/postrinch-128.png")),
        (256, include_bytes!("../assets/icons/postrinch-256.png")),
        (512, include_bytes!("../assets/icons/postrinch-512.png")),
    ],
    svg: include_bytes!("../assets/icons/postrinch.svg"),
};

/// Where keeping current stands, for Settings › About.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// A build folder's window, not the installed one.
    NotInstalled,
    /// Installed; the first check is a minute after start.
    Waiting,
    Checking,
    UpToDate {
        at: String,
    },
    /// Downloaded and verified, waiting for a restart.
    Ready {
        version: String,
    },
    Failed {
        reason: String,
        at: String,
    },
}

/// Wakes the checker for "Check now".
static WAKE: Mutex<Option<Sender<()>>> = Mutex::new(None);

/// Offer an update already staged, and check in the background from now on.
pub fn start(state: State) {
    let Some(dir) = managed_dir() else {
        state.update.set(UpdateStatus::NotInstalled);
        return;
    };
    state.update.set(UpdateStatus::Waiting);
    // What an earlier window staged (one this version reached is gone by
    // now: `postrinch_update::at_start`).
    if let Some(version) = staged_version(&dir) {
        state.update_ready.set(Some(version.clone()));
        state.update.set(UpdateStatus::Ready { version });
    }
    let (wake, woken) = channel::<()>();
    *WAKE.lock().expect("the wake lock") = Some(wake);
    let (status, ready) = (state.update, state.update_ready);
    let spawned = std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            let report = |s: UpdateStatus| status.update_send(move |old| *old = s);
            let _ = woken.recv_timeout(FIRST_CHECK);
            loop {
                report(UpdateStatus::Checking);
                match check_and_stage(&dir) {
                    Ok(Some(version)) => {
                        tracing::info!("update {version} is ready");
                        let shown = version.clone();
                        ready.update_send(move |r| *r = Some(shown));
                        report(UpdateStatus::Ready { version });
                    }
                    // Nothing newer to fetch; one may still be waiting.
                    Ok(None) => match staged_version(&dir) {
                        Some(version) => report(UpdateStatus::Ready { version }),
                        None => report(UpdateStatus::UpToDate { at: now() }),
                    },
                    Err(reason) => {
                        tracing::warn!("update check: {reason}");
                        report(UpdateStatus::Failed { reason, at: now() });
                    }
                }
                let _ = woken.recv_timeout(CHECK_EVERY);
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot check for updates: {e}");
    }
}

fn now() -> String {
    jiff::Zoned::now()
        .strftime("%-d %b %Y at %H:%M")
        .to_string()
}

/// "Check now": run a check at once.
pub fn check_now() {
    if let Some(wake) = WAKE.lock().expect("the wake lock").as_ref() {
        let _ = wake.send(());
    }
}

/// "Restart to update": put the new programs in place, start the new
/// window and close this one.
pub fn restart(state: State) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match postrinch_update::apply(&args) {
        Ok(version) => {
            tracing::info!("restarting into {version}");
            close_current_window();
        }
        Err(reason) => {
            tracing::warn!("restart to update: {reason}");
            state.update_ready.set(None);
            state.update.set(UpdateStatus::Failed {
                reason: format!("The update could not be put in place: {reason}"),
                at: now(),
            });
        }
    }
}

/// `postrinch --install`.
pub fn install() -> Result<String, String> {
    if cfg!(feature = "debug") {
        return Err(
            "this build has rinch's debug server in it (the `debug` feature), which shows \
                    the window's mail to any local program: build without it to install"
                .into(),
        );
    }
    let dir = postrinch_update::install(&ICONS)?;
    Ok(format!(
        "Installed Postrinch {} in {}. Start it from the menu, or as `postrinch`.",
        postrinch_update::CURRENT,
        dir.display()
    ))
}
