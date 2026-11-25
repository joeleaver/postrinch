//! Thunderbird's account settings on this computer, for importing them
//! into Postrinch (docs/imap.md, "Adding an account").
//!
//! [`find`] reads `profiles.ini` in each place Thunderbird keeps its
//! profiles ([`INSTALLS`]: its own package, the Snap and the Flatpak),
//! then each profile's `prefs.js` and, over it, `user.js`, and maps their
//! prefs to [`Account`]s. That is all it reads. Thunderbird's saved
//! passwords and certificates (`logins.json`, `key4.db`, `key3.db`,
//! `signons.sqlite`, `cert9.db`) are never opened, read or looked at:
//! every read goes through [`files::read_settings_file`], which refuses
//! any other file before touching it, and the [`Files`] seam lets tests
//! see each file asked for. Passwords and tokens are never imported.

mod accounts;
pub mod files;
pub mod prefs;
mod profiles;

use std::path::{Path, PathBuf};

pub use accounts::{Account, AccountKind, Identity, Security, Server, SignIn, Signature};
pub use files::{DiskFiles, Files};
pub use prefs::{PrefError, PrefValue, Prefs};
pub use profiles::{ProfileEntry, profiles_in};

/// How Thunderbird is installed, which says where its profiles are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// Its own package or a distribution's: `~/.thunderbird`.
    Native,
    /// The Snap: `~/snap/thunderbird/common/.thunderbird`.
    Snap,
    /// The Flatpak: `~/.var/app/org.mozilla.Thunderbird/.thunderbird`.
    Flatpak,
}

/// Where each installation keeps `profiles.ini`, under the home folder.
pub const INSTALLS: [(Install, &str); 3] = [
    (Install::Native, ".thunderbird"),
    (Install::Snap, "snap/thunderbird/common/.thunderbird"),
    (
        Install::Flatpak,
        ".var/app/org.mozilla.Thunderbird/.thunderbird",
    ),
];

/// Why a profile's settings were not all read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A settings file could not be read: the system's words.
    Unreadable(String),
    /// Some prefs did not parse (and were skipped): how many, and the
    /// first one's line. The rest were read.
    Malformed { prefs: usize, first_line: usize },
}

/// One Thunderbird profile and the accounts in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub install: Install,
    pub name: String,
    pub dir: PathBuf,
    /// Thunderbird opens this profile when it starts.
    pub default: bool,
    pub accounts: Vec<Account>,
    pub problem: Option<Problem>,
}

/// What [`find`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// Each installation's profiles in turn, its default first.
    pub profiles: Vec<Profile>,
    /// The `profiles.ini` files there but not readable, with why.
    pub unreadable: Vec<(PathBuf, String)>,
}

/// Thunderbird's profiles under `home`, from the disk.
pub fn find(home: &Path) -> Found {
    find_with(home, &DiskFiles)
}

/// Thunderbird's profiles under `home`, read through `files`.
pub fn find_with(home: &Path, files: &dyn Files) -> Found {
    let mut found = Found::default();
    for (install, under) in INSTALLS {
        let root = home.join(under);
        let ini = root.join("profiles.ini");
        let text = match files::read_settings_file(files, &ini) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(e) => {
                found.unreadable.push((ini, e.to_string()));
                continue;
            }
        };
        let mut entries = profiles_in(&root, &text);
        // The default first; the rest as profiles.ini lists them.
        entries.sort_by_key(|entry| !entry.default);
        for entry in entries {
            // The same folder listed twice (or by two installations) is
            // one profile.
            if found.profiles.iter().any(|p| p.dir == entry.dir) {
                continue;
            }
            if let Some(profile) = read_profile(install, entry, files) {
                found.profiles.push(profile);
            }
        }
    }
    found
}

/// A profile's accounts, from its `prefs.js` and then its `user.js`
/// (whose prefs win, as they do when Thunderbird starts). A profile
/// whose folder holds no `prefs.js` is one Thunderbird has not used, or
/// whose folder is gone: it is left out.
fn read_profile(install: Install, entry: ProfileEntry, files: &dyn Files) -> Option<Profile> {
    let ProfileEntry { name, dir, default } = entry;
    let mut problem = None;
    let mut prefs = match files::read_settings_file(files, &dir.join("prefs.js")) {
        Ok(Some(text)) => Prefs::parse(&text),
        Ok(None) => return None,
        Err(e) => {
            problem = Some(Problem::Unreadable(e.to_string()));
            Prefs::default()
        }
    };
    match files::read_settings_file(files, &dir.join("user.js")) {
        Ok(Some(text)) => prefs.read(&text),
        Ok(None) => {}
        Err(e) => {
            problem.get_or_insert(Problem::Unreadable(e.to_string()));
        }
    }
    if problem.is_none()
        && let Some(first) = prefs.errors.first()
    {
        problem = Some(Problem::Malformed {
            prefs: prefs.errors.len(),
            first_line: first.line,
        });
    }
    Some(Profile {
        install,
        name,
        dir,
        default,
        accounts: accounts::accounts(&prefs),
        problem,
    })
}
