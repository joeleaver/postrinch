//! Postrinch in Apps & features: the entries under the user's
//! `...\CurrentVersion\Uninstall`. The Setup's is its own (its `AppId`,
//! with `_is1`), removed by its uninstaller; an install from the zip makes
//! one of its own, [`OWN`], whose uninstaller is `postrinch.exe --uninstall`.
//! Only one stands for an install: a zip install over a Setup install keeps
//! the Setup's, and the Setup removes [`OWN`] when it installs over a zip
//! install. An update says its version in whichever is there.

use std::path::Path;

use windows_registry::Key;

use super::not_found;

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
/// The Setup's entry: Inno Setup names it after `AppId` in
/// `tools/windows/postrinch.iss`, which never changes.
pub(crate) const SETUP: &str = "{18A87A0C-8DA8-4CF8-98D3-AE6B5E169DF9}_is1";
/// The zip install's entry.
pub(crate) const OWN: &str = "Postrinch";

fn key_path(name: &str) -> String {
    format!(r"{UNINSTALL}\{name}")
}

/// Did the Setup install Postrinch (and its uninstaller still stand)?
pub(crate) fn setup_installed(user: &Key) -> bool {
    user.open(key_path(SETUP)).is_ok()
}

/// Say `version` in each Apps & features entry there is; make none.
pub(crate) fn record_version(user: &Key, version: &str) -> Result<(), String> {
    for name in [SETUP, OWN] {
        let path = key_path(name);
        let key = match user.options().read().write().open(&path) {
            Ok(key) => key,
            Err(e) if not_found(&e) => continue,
            Err(e) => return Err(format!("opening HKCU\\{path}: {e}")),
        };
        key.set_string("DisplayVersion", version)
            .map_err(|e| format!("writing the version to HKCU\\{path}: {e}"))?;
    }
    Ok(())
}

/// Make (or bring up to date) the zip install's entry for `dir`.
pub(crate) fn register(user: &Key, dir: &Path, version: &str) -> Result<(), String> {
    let path = key_path(OWN);
    let failed = |e: windows::core::Error| format!("writing HKCU\\{path}: {e}");
    let key = user.create(&path).map_err(failed)?;
    let exe = dir.join(crate::exe(crate::PROGRAMS[0]));
    let uninstall = format!("\"{}\" --uninstall", exe.display());
    let strings = [
        ("DisplayName", "Postrinch".to_owned()),
        ("DisplayVersion", version.to_owned()),
        ("Publisher", "LBME Publishing".to_owned()),
        ("DisplayIcon", exe.display().to_string()),
        ("InstallLocation", dir.display().to_string()),
        ("UninstallString", uninstall.clone()),
        ("QuietUninstallString", format!("{uninstall} --quiet")),
        ("URLInfoAbout", "https://postrinch.jkbase.app".to_owned()),
        (
            "HelpLink",
            "https://github.com/joeleaver/postrinch/issues".to_owned(),
        ),
    ];
    for (name, value) in strings {
        key.set_string(name, value).map_err(failed)?;
    }
    // Nothing to change or repair: the window keeps itself current.
    key.set_u32("NoModify", 1).map_err(failed)?;
    key.set_u32("NoRepair", 1).map_err(failed)?;
    key.set_u32("EstimatedSize", size_kib(dir))
        .map_err(failed)?;
    Ok(())
}

/// Remove the zip install's entry.
pub(crate) fn deregister(user: &Key) -> Result<(), String> {
    let path = key_path(OWN);
    match user.remove_tree(&path) {
        Ok(()) => Ok(()),
        Err(e) if not_found(&e) => Ok(()),
        Err(e) => Err(format!("removing HKCU\\{path}: {e}")),
    }
}

/// The three programs' size, in KiB as Apps & features reads it.
fn size_kib(dir: &Path) -> u32 {
    let bytes: u64 = crate::PROGRAMS
        .iter()
        .filter_map(|name| std::fs::metadata(dir.join(crate::exe(name))).ok())
        .map(|m| m.len())
        .sum();
    u32::try_from(bytes / 1024).unwrap_or(u32::MAX)
}
