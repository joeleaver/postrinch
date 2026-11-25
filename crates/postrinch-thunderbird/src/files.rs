//! The only way this crate reads a file. Every read goes through
//! [`read_settings_file`], which refuses any file but `profiles.ini`,
//! `prefs.js` and `user.js` before anything is opened, so Thunderbird's
//! saved passwords (`logins.json`, `key4.db`, `key3.db`,
//! `signons.sqlite`) and certificates (`cert9.db`) are never opened, read
//! or even looked at. Nothing here lists a directory or asks for a file's
//! metadata: a file is opened by name or found missing.

use std::io::{self, Read};
use std::path::Path;

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
        return Err(refused(path));
    }
    files.read(path)
}

fn refused(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "{} is not one of Thunderbird's settings files",
            path.display()
        ),
    )
}

/// The disk. It too refuses anything but the [`SETTINGS_FILES`], so no
/// caller can reach another file through it.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskFiles;

impl Files for DiskFiles {
    fn read(&self, path: &Path) -> io::Result<Option<String>> {
        if !is_settings_file(path) {
            return Err(refused(path));
        }
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
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
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE + 1).unwrap();
        let e = DiskFiles.read(&path).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }
}
