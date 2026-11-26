//! A program file that is busy for a moment. On Windows another process
//! may hold a file open without letting it be moved or deleted (Defender
//! scanning a freshly written program, an indexer, a backup), and a rename
//! or delete then fails with a sharing violation or access denied until it
//! lets go. Every step of an update or uninstall that moves or removes a
//! program goes through [`retry`], which tries again for a few seconds
//! before giving up with the error. Elsewhere the first answer stands.

use std::io;
use std::time::{Duration, Instant};

/// How long a busy file is waited for.
pub(crate) const PATIENCE: Duration = Duration::from_secs(5);
const PAUSE: Duration = Duration::from_millis(50);

/// `op`, tried again while the file it acts on is busy, for [`PATIENCE`].
pub(crate) fn retry<T>(op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_within(PATIENCE, op)
}

/// `op`, tried again while the file it acts on is busy, for `patience`.
pub(crate) fn retry_within<T>(
    patience: Duration,
    mut op: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let deadline = Instant::now() + patience;
    loop {
        match op() {
            Err(e) if busy(&e) && Instant::now() < deadline => std::thread::sleep(PAUSE),
            done => return done,
        }
    }
}

/// Is `e` Windows saying another process has the file?
fn busy(e: &io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    cfg!(windows)
        && matches!(
            e.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
        )
}

#[cfg(all(test, windows))]
pub(crate) mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    /// Hold `path` open as a scanner does, letting others read it but not
    /// move or delete it, until `after` has passed.
    pub(crate) fn hold(path: &Path, after: Duration) -> std::thread::JoinHandle<()> {
        const FILE_SHARE_READ: u32 = 1;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
            .unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            drop(file);
        })
    }

    #[test]
    fn a_file_held_for_a_moment_is_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        std::fs::write(&from, "x").unwrap();
        let held = hold(&from, Duration::from_millis(300));
        assert_eq!(
            std::fs::rename(&from, &to).unwrap_err().raw_os_error(),
            Some(32),
            "the hold refuses a rename"
        );
        retry(|| std::fs::rename(&from, &to)).unwrap();
        held.join().unwrap();
        assert!(to.exists());
    }

    #[test]
    fn a_file_held_too_long_gives_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("held");
        std::fs::write(&path, "x").unwrap();
        let held = hold(&path, Duration::from_secs(2));
        let started = Instant::now();
        let removed = retry_within(Duration::from_millis(200), || std::fs::remove_file(&path));
        assert_eq!(removed.unwrap_err().raw_os_error(), Some(32));
        assert!(started.elapsed() < Duration::from_secs(2));
        held.join().unwrap();
    }

    #[test]
    fn other_errors_are_not_waited_on() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let missing = retry(|| std::fs::remove_file(dir.path().join("missing")));
        assert_eq!(missing.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(started.elapsed() < PATIENCE);
    }
}
