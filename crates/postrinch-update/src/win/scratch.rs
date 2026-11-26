//! A scratch HKEY_CURRENT_USER and Start menu for the tests: a key of its
//! own under the real user's `Software`, and a temporary folder, both
//! removed when the test ends.

use std::sync::atomic::{AtomicU32, Ordering};

use windows_registry::{CURRENT_USER, Key};

use super::Places;

pub(crate) struct Scratch {
    pub places: Places,
    name: String,
    _programs: tempfile::TempDir,
}

impl Scratch {
    pub fn new() -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            r"Software\Postrinch-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let user = CURRENT_USER.create(&name).expect("a scratch key");
        let programs = tempfile::tempdir().unwrap();
        Scratch {
            places: Places {
                user,
                programs: programs.path().join("Programs"),
                broadcast: false,
            },
            name,
            _programs: programs,
        }
    }

    pub fn user(&self) -> &Key {
        &self.places.user
    }

    /// The scratch user's PATH, as written.
    pub fn path(&self) -> Option<String> {
        let env = self.user().open("Environment").ok()?;
        env.get_hstring("Path").ok().map(|p| p.to_string_lossy())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = CURRENT_USER.remove_tree(&self.name);
    }
}
