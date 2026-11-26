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
/// goes to the temporary folder under this name. A per-user uninstall
/// cannot ask Windows to delete it at the next restart (that takes an
/// administrator), so it stays until Windows' own clean-up of the
/// temporary folder (Storage Sense, Disk Cleanup) or Postrinch's next
/// start, if it is installed again ([`clear_leftovers`]).
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
    // The other programs first: one still in use stops the uninstall with
    // nothing else changed. The marker goes last, after the registry and
    // the shortcut, so an uninstall that fails part way can be run again.
    remove_programs(&dir)?;
    deintegrate(&places, &dir)?;
    remove_files(&dir)?;
    let mut said = "Postrinch was removed. Your mail and settings are kept, so installing it \
                    again picks up where you left off."
        .to_owned();
    let own = dir.join(exe(PROGRAMS[0]));
    if let Err(e) = set_aside(&own) {
        said.push_str(&format!(
            " {} is still there ({e}): delete it by hand.",
            own.display()
        ));
    } else if let Err(e) = std::fs::remove_dir(&dir) {
        let empty = std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none());
        said.push_str(&if empty {
            format!(" The empty folder {} is still there ({e}).", dir.display())
        } else {
            format!(
                " {} holds files Postrinch did not put there, so it was left.",
                dir.display()
            )
        });
    }
    Ok(said)
}

/// The programs beside the running window. One that cannot be removed (an
/// agent's app has the bridge open) stops the uninstall.
pub(crate) fn remove_programs(dir: &Path) -> Result<(), String> {
    for name in &PROGRAMS[1..] {
        let path = dir.join(exe(name));
        if path.exists() {
            crate::busy::retry(|| std::fs::remove_file(&path)).map_err(|e| {
                format!(
                    "{} cannot be removed ({e}): close any agent's app that uses it, then try again",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

/// The rest of what the install and the updater put in `dir`, the marker
/// among it, but the running window.
pub(crate) fn remove_files(dir: &Path) -> Result<(), String> {
    let stage = dir.join(STAGE);
    if stage.exists() {
        crate::busy::retry(|| std::fs::remove_dir_all(&stage))
            .map_err(|e| format!("removing {}: {e}", stage.display()))?;
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let left_over = path
            .extension()
            .is_some_and(|ext| ext == "old" || ext == "incoming");
        if left_over || entry.file_name() == MARKER {
            crate::busy::retry(|| std::fs::remove_file(&path))
                .map_err(|e| format!("removing {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// Move the running `exe` out of the install, to the temporary folder
/// ([`SET_ASIDE`] says when it goes from there).
fn set_aside(exe: &Path) -> Result<(), String> {
    use windows::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
    let aside = std::env::temp_dir().join(format!("{SET_ASIDE}{}.exe", std::process::id()));
    let _ = std::fs::remove_file(&aside);
    crate::busy::retry(|| std::fs::rename(exe, &aside)).map_err(|e| e.to_string())?;
    let aside_w = wide(&aside);
    // SAFETY: a NUL-terminated path that outlives the call. Asking Windows
    // to delete it at the next restart needs an administrator's rights, so
    // for a per-user install this normally fails and the file waits for
    // the temporary folder's clean-up.
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
