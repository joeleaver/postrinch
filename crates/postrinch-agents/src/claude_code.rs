//! Finding the terminal's Claude Code (`claude`), which a rule starts to
//! ask an agent. The one place Postrinch looks for it.
//!
//! On Windows, `std::process::Command` finds only `.exe` files on the PATH,
//! while npm installs Claude Code as `claude.cmd`, a batch file that runs
//! its JavaScript entry point with `node`. Rust refuses to pass arguments it
//! cannot quote safely to a batch file (CVE-2024-24576), and a rule's prompt
//! is any text, so the shim is read instead: its target is started directly,
//! through `node` when it is JavaScript, never through `cmd.exe`. The native
//! installer's `claude.exe` is started as it is.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// How to start Claude Code: `program`, then `args`, then Claude's own
/// arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl Launch {
    /// A command starting Claude Code, ready for Claude's own arguments.
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

/// What the person is told when Claude Code is not to be found.
pub const NOT_FOUND: &str = "A rule asked an agent, but Claude Code (the terminal's `claude`) \
     is not installed here, so it could not be started. Install Claude Code, then run the rule again.";

/// Which system's way of installing programs to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Windows,
    Unix,
}

/// Where to look: the PATH, then the folders installers put Claude Code in
/// that a window started before the install may not have on its PATH.
#[derive(Debug, Clone)]
pub(crate) struct Places {
    pub path: Option<OsString>,
    pub home: Option<PathBuf>,
    /// Windows' `%APPDATA%`, where npm installs global programs.
    pub app_data: Option<PathBuf>,
}

impl Places {
    fn here() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            home: dirs::home_dir(),
            app_data: std::env::var_os("APPDATA").map(PathBuf::from),
        }
    }

    /// The folders to look in, in order.
    fn folders(&self, os: Os) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = self
            .path
            .as_deref()
            .map(|p| std::env::split_paths(p).collect())
            .unwrap_or_default();
        if let Some(home) = &self.home {
            folders.push(home.join(".local").join("bin"));
            if os == Os::Unix {
                // Where `claude migrate-installer` put it.
                folders.push(home.join(".claude").join("local"));
            }
        }
        if os == Os::Windows
            && let Some(app_data) = &self.app_data
        {
            folders.push(app_data.join("npm"));
        }
        folders.retain(|f| !f.as_os_str().is_empty());
        folders
    }
}

/// Claude Code on this computer, or `None` when it is not installed.
pub fn find() -> Option<Launch> {
    let os = if cfg!(windows) { Os::Windows } else { Os::Unix };
    find_in(&Places::here(), os)
}

pub(crate) fn find_in(places: &Places, os: Os) -> Option<Launch> {
    let folders = places.folders(os);
    match os {
        Os::Unix => folders
            .iter()
            .map(|f| f.join("claude"))
            .find(|p| is_program(p))
            .map(|program| Launch {
                program,
                args: Vec::new(),
            }),
        Os::Windows => folders.iter().find_map(|f| in_windows_folder(f, &folders)),
    }
}

/// Claude Code in one folder on Windows: its own `claude.exe`, or npm's
/// `claude.cmd`. `folders` are where `node.exe` is looked for when the shim
/// has none beside it, as the shim itself would.
fn in_windows_folder(folder: &Path, folders: &[PathBuf]) -> Option<Launch> {
    let exe = folder.join("claude.exe");
    if exe.is_file() {
        return Some(Launch {
            program: exe,
            args: Vec::new(),
        });
    }
    let shim = folder.join("claude.cmd");
    let text = std::fs::read_to_string(&shim).ok()?;
    let target = shim_target(&text, folder)?;
    if !is_javascript(&target) {
        return Some(Launch {
            program: target,
            args: Vec::new(),
        });
    }
    let node = std::iter::once(folder)
        .chain(folders.iter().map(PathBuf::as_path))
        .map(|f| f.join("node.exe"))
        .find(|p| p.is_file())?;
    Some(Launch {
        program: node,
        args: vec![target.into_os_string()],
    })
}

/// The program an npm shim (`cmd-shim`) runs: the last quoted path under
/// its own folder (`"%dp0%\…"`) that is not `node.exe` and is there.
fn shim_target(text: &str, folder: &Path) -> Option<PathBuf> {
    const DP0: &str = "\"%dp0%\\";
    text.match_indices(DP0)
        .filter_map(|(at, _)| {
            let rest = &text[at + DP0.len()..];
            let inner = &rest[..rest.find('"')?];
            Some(
                inner
                    .split('\\')
                    .filter(|s| !s.is_empty())
                    .fold(folder.to_path_buf(), |path, part| path.join(part)),
            )
        })
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| !n.eq_ignore_ascii_case("node.exe"))
        })
        .filter(|p| p.is_file())
        .last()
}

fn is_javascript(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|e| {
        ["js", "mjs", "cjs"]
            .iter()
            .any(|j| e.eq_ignore_ascii_case(j))
    })
}

#[cfg(unix)]
fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_program(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
#[path = "claude_code_tests.rs"]
mod tests;
