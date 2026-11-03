//! The install: a folder of the person's own holding the three programs and
//! the marker, plus (on Linux) links on the PATH and a menu entry.

use std::path::{Path, PathBuf};

use crate::desktop::{self, Icons};
use crate::{CURRENT, MARKER, PROGRAMS, exe};

/// The folder `--install` puts Postrinch in: `~/.local/share/postrinch-app`
/// on Linux, `%LOCALAPPDATA%\Programs\Postrinch` on Windows. Mail and
/// settings live elsewhere, so removing one never touches the other.
pub fn install_dir() -> Option<PathBuf> {
    let base = dirs::data_local_dir()?;
    Some(if cfg!(windows) {
        base.join("Programs").join("Postrinch")
    } else {
        base.join("postrinch-app")
    })
}

/// The managed install the running program is in, if it is in one.
pub fn managed_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?.to_path_buf();
    dir.join(MARKER).is_file().then_some(dir)
}

/// `postrinch --install`: copy the three programs from the running one's
/// folder into [`install_dir`], mark it, and put it on the PATH and in the
/// menu. Answers the folder.
pub fn install(icons: &Icons) -> Result<PathBuf, String> {
    let dir = install_dir().ok_or("this system has no local data folder to install into")?;
    let exe_path = std::env::current_exe().map_err(|e| format!("finding this program: {e}"))?;
    let from = exe_path.parent().ok_or("this program has no folder")?;
    let missing: Vec<String> = PROGRAMS
        .iter()
        .map(|name| exe(name))
        .filter(|name| !from.join(name).is_file())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{} must be beside this program in {}: the three are installed together",
            missing.join(" and "),
            from.display()
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let same = |a: &Path, b: &Path| {
        a.canonicalize()
            .ok()
            .is_some_and(|a| b.canonicalize().ok() == Some(a))
    };
    if !same(from, &dir) {
        for name in PROGRAMS.map(exe) {
            copy_over(&from.join(&name), &dir.join(&name))?;
        }
    }
    write_marker(&dir, CURRENT)?;
    desktop::integrate(&dir, icons)?;
    Ok(dir)
}

pub(crate) fn write_marker(dir: &Path, version: &str) -> Result<(), String> {
    let body = serde_json::json!({ "version": version }).to_string();
    std::fs::write(dir.join(MARKER), body).map_err(|e| format!("writing the install marker: {e}"))
}

/// Copy `from` to `to` through a file beside `to`, so a running `to` is
/// never half written.
fn copy_over(from: &Path, to: &Path) -> Result<(), String> {
    let tmp = to.with_extension("incoming");
    std::fs::copy(from, &tmp).map_err(|e| format!("copying {}: {e}", from.display()))?;
    replace_file(&tmp, to)
}

/// Move `new` over `dest`. On Linux and macOS a rename: a running `dest`
/// keeps its open file. Windows will not replace a running program, but it
/// will rename one, so the old file steps aside to `*.old` first (removed
/// at the next start).
pub(crate) fn replace_file(new: &Path, dest: &Path) -> Result<(), String> {
    if cfg!(windows) && dest.exists() {
        let old = dest.with_extension("old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(dest, &old).map_err(|e| format!("moving {} aside: {e}", dest.display()))?;
    }
    std::fs::rename(new, dest).map_err(|e| format!("putting {} in place: {e}", dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("making {} runnable: {e}", dest.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_replaced_whole_and_runnable() {
        let dir = tempfile::tempdir().unwrap();
        let (new, dest) = (dir.path().join("new"), dir.path().join("dest"));
        std::fs::write(&dest, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        replace_file(&new, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "new");
        assert!(!new.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }
}
