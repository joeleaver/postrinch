//! A content-addressed directory of raw messages.
//!
//! A blob is named by the SHA-256 of its bytes and written to a temporary
//! file, synced, then renamed into place, so a reader never sees a partial
//! blob and a crash leaves at most a stray temporary file, which the next
//! open removes.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

const TMP_SUFFIX: &str = ".tmp";
static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct Blobs {
    dir: PathBuf,
}

impl Blobs {
    pub(crate) fn open(dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let blobs = Self { dir };
        blobs.remove_stray_temporaries()?;
        Ok(blobs)
    }

    /// Store `bytes`, returning their SHA-256 in hex. Storing the same bytes
    /// twice writes them once.
    pub(crate) fn put(&self, bytes: &[u8]) -> io::Result<String> {
        let hash = hex(&Sha256::digest(bytes));
        let path = self.path_of(&hash);
        if path.exists() {
            return Ok(hash);
        }
        let parent = path.parent().expect("blob paths have a parent");
        fs::create_dir_all(parent)?;
        // Unique per process and per call, so two writers of the same bytes
        // never share a temporary file.
        let n = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!("{hash}.{}-{n}{TMP_SUFFIX}", std::process::id()));
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(hash)
    }

    pub(crate) fn get(&self, hash: &str) -> io::Result<Option<Vec<u8>>> {
        if !is_hash(hash) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a blob hash: {hash:?}"),
            ));
        }
        match fs::read(self.path_of(hash)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete every blob whose hash `keep` rejects, except blobs modified
    /// within `grace`: a writer stores the blob before the message that
    /// refers to it, and must not lose it in between. Returns how many were
    /// deleted.
    pub(crate) fn retain(&self, keep: impl Fn(&str) -> bool, grace: Duration) -> io::Result<usize> {
        let now = SystemTime::now();
        let mut deleted = 0;
        for shard in fs::read_dir(&self.dir)? {
            let shard = shard?.path();
            let Some(prefix) = shard
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if !shard.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&shard)? {
                let entry = entry?;
                let Some(rest) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let hash = format!("{prefix}{rest}");
                if !is_hash(&hash) || keep(&hash) {
                    continue;
                }
                let modified = entry.metadata()?.modified()?;
                if now.duration_since(modified).unwrap_or_default() < grace {
                    continue;
                }
                fs::remove_file(entry.path())?;
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    /// `ab/cdef…`: two levels keep any one directory small.
    fn path_of(&self, hash: &str) -> PathBuf {
        self.dir.join(&hash[..2]).join(&hash[2..])
    }

    fn remove_stray_temporaries(&self) -> io::Result<()> {
        for shard in fs::read_dir(&self.dir)? {
            let shard = shard?.path();
            if !shard.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&shard)? {
                let path = entry?.path();
                if is_temporary(&path) {
                    fs::remove_file(&path)?;
                }
            }
        }
        Ok(())
    }
}

fn is_temporary(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(TMP_SUFFIX))
}

fn is_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(dir.path().to_path_buf()).unwrap();
        let a = blobs.put(b"From: ada\r\n\r\nhello").unwrap();
        let again = blobs.put(b"From: ada\r\n\r\nhello").unwrap();
        assert_eq!(a, again);
        assert_eq!(blobs.get(&a).unwrap().unwrap(), b"From: ada\r\n\r\nhello");
        let missing = "0".repeat(64);
        assert_eq!(blobs.get(&missing).unwrap(), None);
        assert!(blobs.get("../etc/passwd").is_err());
    }

    #[test]
    fn retain_deletes_unkept_blobs_past_grace() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(dir.path().to_path_buf()).unwrap();
        let kept = blobs.put(b"kept").unwrap();
        let dropped = blobs.put(b"dropped").unwrap();

        assert_eq!(
            blobs
                .retain(|h| h == kept, Duration::from_secs(3600))
                .unwrap(),
            0
        );
        assert_eq!(blobs.retain(|h| h == kept, Duration::ZERO).unwrap(), 1);
        assert!(blobs.get(&kept).unwrap().is_some());
        assert!(blobs.get(&dropped).unwrap().is_none());
    }

    #[test]
    fn open_removes_stray_temporaries() {
        let dir = tempfile::tempdir().unwrap();
        let shard = dir.path().join("ab");
        fs::create_dir_all(&shard).unwrap();
        let stray = shard.join("abcd.123.tmp");
        fs::write(&stray, b"partial").unwrap();
        Blobs::open(dir.path().to_path_buf()).unwrap();
        assert!(!stray.exists());
    }
}
