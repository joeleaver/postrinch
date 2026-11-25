//! What the tests share: the fixture homes, a [`Files`] that notes each
//! file asked for, and copying a fixture into a scratch home.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};

use postrinch_thunderbird::{DiskFiles, Files};

/// The fixture home `name`, under `tests/fixtures`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The disk, noting each file asked for, in order.
#[derive(Default)]
pub struct Recording {
    asked: RefCell<Vec<PathBuf>>,
}

impl Recording {
    pub fn asked(&self) -> Vec<PathBuf> {
        self.asked.borrow().clone()
    }
}

impl Files for Recording {
    fn read(&self, path: &Path) -> io::Result<Option<String>> {
        self.asked.borrow_mut().push(path.to_owned());
        DiskFiles.read(path)
    }
}

/// Copies the folder `from` into `to`, made as needed.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}
