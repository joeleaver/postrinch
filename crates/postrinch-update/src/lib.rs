//! Postrinch installed in the person's own folders, kept current from
//! signed GitHub releases (DESIGN.md, "Installing and updating").
//!
//! `postrinch --install` copies the three programs (the window,
//! `postrinch-mcp` and `postrinch-cli`) into [`install_dir`] and marks it. A
//! window running from a marked folder checks the latest release, downloads
//! and verifies its package, and stages it inside the install; "Restart to
//! update" moves all three into place together, because the bridge and the
//! app speak a wire format that must match. Nothing here acts for a program
//! outside a marked folder, so a development build never updates itself.

mod apply;
mod check;
mod desktop;
mod install;

pub use apply::{apply, at_start};
pub use check::{check_and_stage, staged_version};
pub use desktop::Icons;
pub use install::{install, install_dir, managed_dir};

/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The file that makes a folder a managed install: `{"version": "…"}`.
const MARKER: &str = "postrinch-install.json";
/// Where a downloaded update waits for a restart, inside the install.
const STAGE: &str = "update";
/// Names the process a restarted window waits for before it opens the store.
const WAIT_ENV: &str = "POSTRINCH_WAIT_FOR_PID";

const EXE_SUFFIX: &str = std::env::consts::EXE_SUFFIX;

/// The window, then the programs that must always match it.
const PROGRAMS: [&str; 3] = ["postrinch", "postrinch-mcp", "postrinch-cli"];

/// `name` with this system's executable suffix.
fn exe(name: &str) -> String {
    format!("{name}{EXE_SUFFIX}")
}

/// Is `version` later than this build?
fn newer_than_current(version: &str) -> bool {
    newer_than(version, CURRENT)
}

fn newer_than(version: &str, current: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim_start_matches('v')).ok();
    match (parse(version), parse(current)) {
        (Some(v), Some(current)) => v > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_later_version_is_an_update() {
        assert!(newer_than("v0.2.0", "0.1.0"));
        assert!(newer_than("0.1.1", "0.1.0"));
        assert!(!newer_than("0.1.0", "0.1.0"));
        assert!(!newer_than("0.0.9", "0.1.0"));
        assert!(!newer_than("garbage", "0.1.0"));
        assert!(!newer_than_current(CURRENT));
    }
}
