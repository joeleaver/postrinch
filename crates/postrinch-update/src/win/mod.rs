//! Windows: what the Setup (`tools/windows/postrinch.iss`) leaves besides
//! the files, done the same way by `postrinch.exe --install` from the zip,
//! so both routes end in the same install: a Start-menu entry, the folder on
//! the person's PATH, and an entry in Apps & features.
//!
//! Every write goes through [`Places`], which names the registry key that
//! stands for HKEY_CURRENT_USER and the Start menu's Programs folder; the
//! tests point it at a scratch key and a temporary folder.

pub(crate) mod path;
pub(crate) mod registration;
pub(crate) mod shortcut;
pub mod uninstall;

use std::path::{Path, PathBuf};

use windows_registry::{CURRENT_USER, Key};

/// The Start-menu entry's file name, the one the Setup's `[Icons]` makes.
pub(crate) const SHORTCUT: &str = "Postrinch.lnk";
/// Postrinch's own key under the user's `Software`: what the install did
/// that its uninstall must undo, and nothing else.
pub(crate) const STATE_KEY: &str = r"Software\Postrinch";

/// Where an install writes.
pub(crate) struct Places {
    /// HKEY_CURRENT_USER, or a scratch key in a test.
    pub user: Key,
    /// The Start menu's Programs folder.
    pub programs: PathBuf,
    /// Tell running programs the environment changed. Off in a test, whose
    /// scratch `Environment` is no one's.
    pub broadcast: bool,
}

impl Places {
    /// The person's own.
    pub fn user() -> Result<Places, String> {
        let user = CURRENT_USER
            .options()
            .read()
            .write()
            .open("")
            .map_err(|e| format!("opening the registry: {e}"))?;
        Ok(Places {
            user,
            programs: shortcut::programs_folder()?,
            broadcast: true,
        })
    }
}

/// What `--install` adds for `dir` holding `version`. Running it again
/// changes nothing more: one shortcut, one PATH entry, one Apps & features
/// entry. Answers what the person should hear.
pub(crate) fn integrate(places: &Places, dir: &Path, version: &str) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    let exe = dir.join(crate::exe(crate::PROGRAMS[0]));
    shortcut::write(&places.programs.join(SHORTCUT), &exe, dir)?;
    match path::add(&places.user, dir)? {
        path::Added::Now => notes.push(format!(
            "Added {} to your PATH: a terminal opened from now on runs `postrinch-cli` by name.",
            dir.display()
        )),
        path::Added::Before | path::Added::ByThePerson => {}
    }
    if registration::setup_installed(&places.user) {
        // The Setup's entry in Apps & features already stands for this
        // folder: say the new version there rather than make a second one.
        registration::record_version(&places.user, version)?;
    } else {
        registration::register(&places.user, dir, version)?;
    }
    if places.broadcast {
        environment_changed();
    }
    Ok(notes)
}

/// Undo [`integrate`] for `dir`: the shortcut, the PATH entry if the
/// install added it (never one the person had), Apps & features' entry and
/// Postrinch's own key.
pub(crate) fn deintegrate(places: &Places, dir: &Path) -> Result<(), String> {
    let lnk = places.programs.join(SHORTCUT);
    if lnk.exists() {
        crate::busy::retry(|| std::fs::remove_file(&lnk))
            .map_err(|e| format!("removing {}: {e}", lnk.display()))?;
    }
    path::remove(&places.user, dir)?;
    registration::deregister(&places.user)?;
    match places.user.remove_tree(STATE_KEY) {
        Ok(()) => {}
        Err(e) if not_found(&e) => {}
        Err(e) => return Err(format!("removing HKCU\\{STATE_KEY}: {e}")),
    }
    if places.broadcast {
        environment_changed();
    }
    Ok(())
}

/// Tell running programs (Explorer first) to read the environment again,
/// so a terminal started from the desktop next sees the new PATH. A program
/// that does not answer within five seconds is not waited on longer.
fn environment_changed() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    let area = wide("Environment");
    // SAFETY: `area` outlives the call, which reads it as a string.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(area.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            None,
        );
    }
}

/// `%VAR%`s expanded the way the system expands a PATH entry.
pub(crate) fn expand(s: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows::core::PCWSTR;
    let src = wide(s);
    let mut out = vec![0u16; s.len() + 256];
    loop {
        // SAFETY: `src` is NUL-terminated and `out` is a writable buffer
        // whose length the call is given.
        let n = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut out)) } as usize;
        if n == 0 {
            return s.to_owned();
        }
        if n <= out.len() {
            return String::from_utf16_lossy(&out[..n - 1]);
        }
        out.resize(n, 0);
    }
}

/// `s` as a NUL-terminated UTF-16 string.
pub(crate) fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

/// Is `e` the registry saying the key or value is not there?
pub(crate) fn not_found(e: &windows::core::Error) -> bool {
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    e.code() == windows::core::HRESULT::from_win32(ERROR_FILE_NOT_FOUND)
}

#[cfg(test)]
pub(crate) mod scratch;

#[cfg(test)]
mod tests;
