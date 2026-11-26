//! `postrinch.exe --uninstall`: what Apps & features runs for an install
//! from the zip (the Setup's install has its own uninstaller). It removes
//! what `--install` put down, the programs and the marker, and what the
//! updater leaves beside them; mail, settings and the keychain's sign-ins
//! live elsewhere and are never touched.

use std::path::Path;

use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW,
};
use windows::core::PCWSTR;

use super::{Places, deintegrate, registration, wide};
use crate::{MARKER, PROGRAMS, STAGE, exe, install_dir, managed_dir};

/// The running `postrinch.exe` cannot be deleted, but it can be moved: it
/// goes to the temporary folder under this name, removed the next time
/// Postrinch starts ([`clear_leftovers`]) or by Windows' own clean-up.
const SET_ASIDE: &str = "postrinch-removed-";

/// Remove the install this program is running from. Answers what to tell
/// the person.
pub fn uninstall() -> Result<String, String> {
    let dir = install_dir().ok_or("this system has no local data folder")?;
    let canonical = |p: &Path| p.canonicalize().ok();
    let here = managed_dir();
    if here.is_none() || here.as_deref().and_then(canonical) != canonical(&dir) {
        return Err(format!(
            "this copy of Postrinch is not the installed one, which is in {}",
            dir.display()
        ));
    }
    let places = Places::user()?;
    if registration::setup_installed(&places.user) {
        return Err(
            "Postrinch was installed by its Setup: remove it in Settings › Apps › \
                    Installed apps, which runs the Setup's own uninstaller"
                .into(),
        );
    }
    if !crate::process::running_from(&dir).is_empty() {
        return Err(
            "Postrinch is still running: close its window, and any agent's app \
                    that uses it, then try again"
                .into(),
        );
    }
    remove_files(&dir)?;
    deintegrate(&places, &dir)?;
    let mut said = "Postrinch was removed. Your mail and settings are kept, so installing it \
                    again picks up where you left off."
        .to_owned();
    let own = dir.join(exe(PROGRAMS[0]));
    if let Err(e) = set_aside(&own) {
        said.push_str(&format!(
            " {} is still there ({e}): delete it by hand.",
            own.display()
        ));
    } else if std::fs::remove_dir(&dir).is_err() {
        said.push_str(&format!(
            " {} holds files Postrinch did not put there, so it was left.",
            dir.display()
        ));
    }
    Ok(said)
}

/// Everything in `dir` that the install and the updater put there, but the
/// running window. Another program in it that cannot be removed stops the
/// uninstall before anything else is.
pub(crate) fn remove_files(dir: &Path) -> Result<(), String> {
    for name in &PROGRAMS[1..] {
        let path = dir.join(exe(name));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| {
                format!(
                    "{} cannot be removed ({e}): close any agent's app that uses it, then try again",
                    path.display()
                )
            })?;
        }
    }
    let stage = dir.join(STAGE);
    if stage.exists() {
        std::fs::remove_dir_all(&stage)
            .map_err(|e| format!("removing {}: {e}", stage.display()))?;
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let left_over = path
            .extension()
            .is_some_and(|ext| ext == "old" || ext == "incoming");
        if left_over || entry.file_name() == MARKER {
            std::fs::remove_file(&path).map_err(|e| format!("removing {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// Move the running `exe` out of the install, to be deleted at the next
/// restart if Windows lets this user ask for that, or else at Postrinch's
/// next start.
fn set_aside(exe: &Path) -> Result<(), String> {
    use windows::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
    let aside = std::env::temp_dir().join(format!("{SET_ASIDE}{}.exe", std::process::id()));
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(exe, &aside).map_err(|e| e.to_string())?;
    let aside_w = wide(&aside);
    // SAFETY: a NUL-terminated path that outlives the call. Asking needs an
    // administrator's rights; without them it fails and `clear_leftovers`
    // is what removes the file.
    let _ = unsafe {
        MoveFileExW(
            PCWSTR(aside_w.as_ptr()),
            PCWSTR::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
    };
    Ok(())
}

/// Delete programs an earlier uninstall set aside.
pub(crate) fn clear_leftovers() {
    let temp = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&temp) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(SET_ASIDE) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Ask the person to confirm the uninstall, as an uninstaller does when
/// Apps & features starts it.
pub fn confirm() -> bool {
    let text = wide(
        "Remove Postrinch from this computer?\n\nYour mail and settings are kept, so \
         installing it again picks up where you left off.",
    );
    let caption = wide("Uninstall Postrinch");
    // SAFETY: NUL-terminated strings that outlive the call.
    let answer = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    answer == IDYES
}

/// Show the person the outcome: an uninstall started from Apps & features
/// has no console to print to.
pub fn tell(text: &str, failed: bool) {
    let text = wide(text);
    let caption = wide("Uninstall Postrinch");
    let icon = if failed {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    // SAFETY: NUL-terminated strings that outlive the call.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK | icon,
        );
    }
}
