//! The only way this crate reads a file. Every read goes through
//! [`read_settings_file`], which refuses any file but `profiles.ini`,
//! `prefs.js` and `user.js` before anything is opened, so Thunderbird's
//! saved passwords (`logins.json`, `key4.db`, `key3.db`,
//! `signons.sqlite`) and certificates (`cert9.db`) are never opened, read
//! or even looked at. Nothing here lists a directory: a file is opened by
//! name or found missing.
//!
//! A settings file that is a symbolic link is refused, never followed: a
//! `user.js` linked to `logins.json` would otherwise read the passwords
//! under an allowed name. On Unix the open itself refuses the link
//! (`O_NOFOLLOW`), so nothing can swap one in between a check and the
//! open. The folders on the way may be links (a profile folder moved
//! elsewhere and linked back is common, and `profiles.ini` may name any
//! folder anyway): what is read from them is still only a settings file,
//! by name, and never a link. A hard link is the file itself under a
//! second name, so it is read like any other.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The files this crate may read, by name.
pub const SETTINGS_FILES: [&str; 3] = ["profiles.ini", "prefs.js", "user.js"];

/// The biggest settings file read: a prefs.js is a few hundred
/// kilobytes at most, so anything bigger is not one.
pub const MAX_FILE: u64 = 16 * 1024 * 1024;

/// Reads files by path. [`DiskFiles`] reads the disk; tests stand in to
/// see which files are asked for.
pub trait Files {
    /// The file's text, or none when there is no such file.
    fn read(&self, path: &Path) -> io::Result<Option<String>>;
}

/// Whether `path` names one of the [`SETTINGS_FILES`].
pub fn is_settings_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SETTINGS_FILES.contains(&name))
}

/// Reads `path` through `files`, if it is one of the [`SETTINGS_FILES`];
/// any other file is refused without being touched.
pub fn read_settings_file(files: &dyn Files, path: &Path) -> io::Result<Option<String>> {
    if !is_settings_file(path) {
        return Err(refused(path, Refusal::NotSettings));
    }
    files.read(path)
}

/// Why a file was not read, though it may be there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// It is not one of the [`SETTINGS_FILES`].
    NotSettings,
    /// It is a symbolic link; what it points to was not opened.
    Link,
    /// It is not a plain file (a folder, a pipe, a device).
    NotPlain,
}

/// A file refused, as the error of its read: [`refusal`] tells it from
/// the system's own errors.
#[derive(Debug)]
struct Refused {
    path: PathBuf,
    why: Refusal,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        match self.why {
            Refusal::NotSettings => write!(
                f,
                "{} is not one of Thunderbird's settings files",
                self.path.display()
            ),
            Refusal::Link => write!(f, "{name} is a link, so Postrinch didn't read it"),
            Refusal::NotPlain => {
                write!(f, "{name} is not a plain file, so Postrinch didn't read it")
            }
        }
    }
}

impl std::error::Error for Refused {}

fn refused(path: &Path, why: Refusal) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        Refused {
            path: path.to_owned(),
            why,
        },
    )
}

/// Why `error` is a refusal of this crate's, or none when it is the
/// system's (a file not allowed to be read, say).
pub fn refusal(error: &io::Error) -> Option<Refusal> {
    error
        .get_ref()?
        .downcast_ref::<Refused>()
        .map(|refused| refused.why)
}

/// The disk. It too refuses anything but the [`SETTINGS_FILES`], so no
/// caller can reach another file through it, and refuses a link.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskFiles;

impl Files for DiskFiles {
    fn read(&self, path: &Path) -> io::Result<Option<String>> {
        if !is_settings_file(path) {
            return Err(refused(path, Refusal::NotSettings));
        }
        let file = match open_unfollowed(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        // Asked of the file opened, not of its name, so nothing can be
        // swapped in between.
        if !file.metadata()?.is_file() {
            return Err(refused(path, Refusal::NotPlain));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is too big to be a settings file", path.display()),
            ));
        }
        // Thunderbird writes its settings as UTF-8; a stray byte becomes
        // U+FFFD rather than losing the whole file.
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

/// Whether `path` itself is a symbolic link (not what it points to).
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Opens `path` to read, refusing it when it is a link. With
/// `O_NOFOLLOW` the open fails on a link rather than following it (with
/// `ELOOP` on Linux, `EMLINK` on FreeBSD), so a failed open is told as a
/// link when the name is one. `O_NONBLOCK` keeps a pipe left under the
/// name from holding the open for ever (it is then refused as not a
/// plain file); a plain file's reads are as without it.
#[cfg(unix)]
fn open_unfollowed(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| {
            if is_link(path) {
                refused(path, Refusal::Link)
            } else {
                e
            }
        })
}

/// Opens `path` to read, refusing it when it is a link. Without
/// `O_NOFOLLOW` the name is looked at first and then opened, which
/// leaves a race: something that can write in the profile's folder could
/// put a link there between the two, and the open would follow it.
/// Postrinch looks for Thunderbird only on Linux ([`crate::INSTALLS`]),
/// so this is only a build elsewhere's.
#[cfg(not(unix))]
fn open_unfollowed(path: &Path) -> io::Result<File> {
    if is_link(path) {
        return Err(refused(path, Refusal::Link));
    }
    File::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_settings_files_are_read() {
        for name in [
            "logins.json",
            "key4.db",
            "key3.db",
            "signons.sqlite",
            "cert9.db",
            "prefs.js.bak",
            "",
        ] {
            let path = Path::new("/nowhere/profile").join(name);
            assert!(!is_settings_file(&path), "{name}");
            let e = DiskFiles.read(&path).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{name}");
            assert_eq!(refusal(&e), Some(Refusal::NotSettings), "{name}");
        }
        for name in SETTINGS_FILES {
            assert!(is_settings_file(&Path::new("/p").join(name)));
        }
    }

    #[test]
    fn a_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.js");
        assert!(DiskFiles.read(&path).unwrap().is_none());
    }

    #[test]
    fn a_file_too_big_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.js");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_FILE + 1).unwrap();
        let e = DiskFiles.read(&path).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_folder_is_not_a_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.ini");
        std::fs::create_dir(&path).unwrap();
        let e = DiskFiles.read(&path).unwrap_err();
        assert_eq!(refusal(&e), Some(Refusal::NotPlain));
        assert_eq!(
            e.to_string(),
            "profiles.ini is not a plain file, so Postrinch didn't read it"
        );
    }

    /// A profile folder holding a readable passwords file: had a link
    /// been followed, its open would succeed and the read would return
    /// the file, so the test below fails unless the link is refused.
    #[cfg(unix)]
    fn decoy() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let logins = dir.path().join("logins.json");
        std::fs::write(&logins, "{\"logins\":[\"secret\"]}").unwrap();
        (dir, logins)
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_settings_file_is_refused_without_opening_what_it_points_to() {
        let (dir, logins) = decoy();
        for name in SETTINGS_FILES {
            let path = dir.path().join(name);
            std::os::unix::fs::symlink(&logins, &path).unwrap();
            let e = DiskFiles.read(&path).unwrap_err();
            assert_eq!(refusal(&e), Some(Refusal::Link), "{name}: {e}");
            assert_eq!(
                e.to_string(),
                format!("{name} is a link, so Postrinch didn't read it")
            );
        }
        // The decoy is what the test says: the same file under a settings
        // file's name, not a link, is read, so only the link was refused.
        let plain = dir.path().join("elsewhere");
        std::fs::create_dir(&plain).unwrap();
        let prefs = plain.join("prefs.js");
        std::fs::copy(&logins, &prefs).unwrap();
        assert!(DiskFiles.read(&prefs).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_refused_not_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user.js");
        std::os::unix::fs::symlink(dir.path().join("gone"), &path).unwrap();
        let e = DiskFiles.read(&path).unwrap_err();
        assert_eq!(refusal(&e), Some(Refusal::Link));
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_is_refused_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.js");
        let made = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(made.success());
        let e = DiskFiles.read(&path).unwrap_err();
        assert_eq!(refusal(&e), Some(Refusal::NotPlain));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_profile_folder_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("prefs.js"), "user_pref(\"a\", 1);").unwrap();
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let text = DiskFiles.read(&linked.join("prefs.js")).unwrap();
        assert_eq!(text.as_deref(), Some("user_pref(\"a\", 1);"));
    }
}
