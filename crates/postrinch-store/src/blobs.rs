//! A content-addressed directory of raw messages.
//!
//! A blob is named by the SHA-256 of its bytes and kept compressed with zstd
//! (`ab/cdef….zst`): raw mail is text, and attachments in it are base64, so
//! it shrinks by about 40%. Reading a blob checks its hash, so a damaged
//! file is an error rather than a wrong message.
//!
//! A blob is written to a temporary file, synced, then renamed into place,
//! so a reader never sees a partial blob and a crash leaves at most a stray
//! temporary file, which the next open removes.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

const TMP_SUFFIX: &str = ".tmp";
const BLOB_SUFFIX: &str = ".zst";
/// zstd's default. Higher levels save under 2% more on real mail, at
/// several times the cost.
const LEVEL: i32 = 3;
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
        let compressed = zstd::encode_all(bytes, LEVEL)?;
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&compressed)?;
            file.sync_all()?;
        }
        match fs::rename(&tmp, &path) {
            Ok(()) => {}
            // Windows will not rename onto an existing file. If another
            // writer stored the same bytes first, theirs is as good as ours.
            Err(_) if path.exists() => {
                fs::remove_file(&tmp)?;
                return Ok(hash);
            }
            Err(e) => return Err(e),
        }
        sync_dir(parent)?;
        Ok(hash)
    }

    pub(crate) fn get(&self, hash: &str) -> io::Result<Option<Vec<u8>>> {
        if !is_hash(hash) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a blob hash: {hash:?}"),
            ));
        }
        let compressed = match fs::read(self.path_of(hash)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let damaged = |why: String| io::Error::new(io::ErrorKind::InvalidData, why);
        let bytes = zstd::decode_all(compressed.as_slice())
            .map_err(|e| damaged(format!("blob {hash} is damaged: {e}")))?;
        if hex(&Sha256::digest(&bytes)) != hash {
            return Err(damaged(format!("blob {hash} is damaged: its hash differs")));
        }
        Ok(Some(bytes))
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
                let name = entry.file_name();
                let Some(rest) = name.to_str().and_then(|n| n.strip_suffix(BLOB_SUFFIX)) else {
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

    /// `ab/cdef….zst`: two levels keep any one directory small.
    fn path_of(&self, hash: &str) -> PathBuf {
        self.dir
            .join(&hash[..2])
            .join(format!("{}{BLOB_SUFFIX}", &hash[2..]))
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

/// Make a rename in `dir` durable. On Unix that takes an fsync of the
/// directory. Windows has no directory fsync, and std's rename there is not
/// write-through, so a power loss just after it can lose the rename: the
/// message then refers to a blob that is not there, which reads as a raw
/// message not yet fetched.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
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
    fn stores_compressed_and_refuses_damage() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(dir.path().to_path_buf()).unwrap();
        let text = b"Subject: rota\r\n\r\n".repeat(200);
        let hash = blobs.put(&text).unwrap();
        let path = blobs.path_of(&hash);
        assert!(fs::metadata(&path).unwrap().len() < text.len() as u64 / 4);

        let other = zstd::encode_all(&b"something else"[..], LEVEL).unwrap();
        fs::write(&path, other).unwrap();
        assert_eq!(
            blobs.get(&hash).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::write(&path, b"not zstd").unwrap();
        assert_eq!(
            blobs.get(&hash).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
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
