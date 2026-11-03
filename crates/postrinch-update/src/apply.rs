//! "Restart to update": the staged programs moved into place together, the
//! new window started, and (at its start) the old one waited out.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::install::{managed_dir, replace_file, write_marker};
use crate::{PROGRAMS, STAGE, WAIT_ENV, exe, newer_than_current, staged_version};

/// How long a restarted window waits for the one it replaces to close. The
/// store takes one opener, so it cannot start before.
const WAIT: Duration = Duration::from_secs(60);

/// First thing in the window, before the store is opened: wait for the
/// process a restart replaces, then tidy what an earlier update left.
pub fn at_start() {
    if let Some(pid) = std::env::var(WAIT_ENV)
        .ok()
        .and_then(|p| p.parse::<u32>().ok())
    {
        // SAFETY: called first in main, before any other thread exists.
        unsafe { std::env::remove_var(WAIT_ENV) };
        let deadline = Instant::now() + WAIT;
        while process_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if let Some(dir) = managed_dir() {
        clean_up(&dir);
    }
}

fn process_alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        Path::new(&format!("/proc/{pid}")).exists()
    } else if cfg!(windows) {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    } else {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }
}

fn clean_up(dir: &Path) {
    for name in PROGRAMS {
        let _ = std::fs::remove_file(dir.join(exe(name)).with_extension("old"));
    }
    // A staged update this version has already reached (installed by hand).
    if staged_version(dir).is_some_and(|v| !newer_than_current(&v)) {
        let _ = std::fs::remove_dir_all(dir.join(STAGE));
    }
}

/// Move the staged programs into place, mark the install with their version
/// and start the new window with `args`. The caller closes its window next.
/// Answers the version started.
pub fn apply(args: &[String]) -> Result<String, String> {
    let dir = managed_dir().ok_or("this copy of Postrinch is not an installed one")?;
    let version = staged_version(&dir).ok_or("no update is ready")?;
    let stage = dir.join(STAGE);
    for name in PROGRAMS {
        replace_file(&stage.join(exe(name)), &dir.join(exe(name)))?;
    }
    write_marker(&dir, &version)?;
    let _ = std::fs::remove_dir_all(&stage);
    std::process::Command::new(dir.join(exe(PROGRAMS[0])))
        .args(args)
        .env(WAIT_ENV, std::process::id().to_string())
        .spawn()
        .map_err(|e| format!("starting {version}: {e}"))?;
    Ok(version)
}
