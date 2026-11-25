//! `profiles.ini`: which profiles a Thunderbird has, where each one's
//! folder is, and which is the default.
//!
//! ```ini
//! [Profile0]
//! Name=default-release
//! IsRelative=1
//! Path=abcd1234.default-release
//! Default=1
//!
//! [Install6F193CCC56814779]
//! Default=abcd1234.default-release
//! Locked=1
//! ```
//!
//! A relative `Path` (`IsRelative=1`) is under the folder holding
//! `profiles.ini`; otherwise it is absolute. The default profile is the
//! one an `[Install…]` section names (as Thunderbird has done since 67,
//! one per installation), or, when no section names one, the profile
//! marked `Default=1`.

use std::path::{Path, PathBuf};

/// One profile listed in `profiles.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEntry {
    pub name: String,
    /// The profile's folder.
    pub dir: PathBuf,
    pub default: bool,
}

/// One `[section]` of an INI file, its keys in order.
struct Section<'a> {
    name: &'a str,
    keys: Vec<(&'a str, &'a str)>,
}

impl<'a> Section<'a> {
    fn get(&self, key: &str) -> Option<&'a str> {
        self.keys.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

fn sections(text: &str) -> Vec<Section<'_>> {
    let mut sections: Vec<Section<'_>> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push(Section {
                name: name.trim(),
                keys: Vec::new(),
            });
        } else if let Some((key, value)) = line.split_once('=')
            && let Some(section) = sections.last_mut()
        {
            section.keys.push((key.trim(), value.trim()));
        }
    }
    sections
}

/// A `[Profile<n>]` section's name.
fn is_profile_section(name: &str) -> bool {
    name.strip_prefix("Profile")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The profiles `text` (a `profiles.ini` in the folder `root`) lists.
pub fn profiles_in(root: &Path, text: &str) -> Vec<ProfileEntry> {
    let sections = sections(text);
    let install_defaults: Vec<PathBuf> = sections
        .iter()
        .filter(|s| s.name.starts_with("Install"))
        .filter_map(|s| s.get("Default"))
        .filter(|path| !path.is_empty())
        .map(|path| root.join(path))
        .collect();
    let mut profiles = Vec::new();
    for section in sections.iter().filter(|s| is_profile_section(s.name)) {
        let Some(path) = section.get("Path").filter(|p| !p.is_empty()) else {
            continue;
        };
        // A relative path is under `root`; an absolute one stands alone.
        let dir = if section.get("IsRelative") == Some("1") {
            root.join(path)
        } else {
            PathBuf::from(path)
        };
        if !dir.is_absolute() {
            // Thunderbird ignores an absolute path that is not one.
            continue;
        }
        let default = if install_defaults.is_empty() {
            section.get("Default") == Some("1")
        } else {
            install_defaults.contains(&dir)
        };
        let name = match section.get("Name").filter(|n| !n.is_empty()) {
            Some(name) => name.to_owned(),
            None => dir
                .file_name()
                .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned()),
        };
        profiles.push(ProfileEntry { name, dir, default });
    }
    profiles
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/home/ada/.thunderbird";

    #[test]
    fn relative_and_absolute_paths() {
        let profiles = profiles_in(
            Path::new(ROOT),
            "[General]\nStartWithLastProfile=1\n\n[Profile1]\nName=Work\nIsRelative=0\nPath=/mnt/work/tb\n\n[Profile0]\nName=default\nIsRelative=1\nPath=abcd.default\nDefault=1\n",
        );
        assert_eq!(
            profiles,
            vec![
                ProfileEntry {
                    name: "Work".into(),
                    dir: "/mnt/work/tb".into(),
                    default: false,
                },
                ProfileEntry {
                    name: "default".into(),
                    dir: "/home/ada/.thunderbird/abcd.default".into(),
                    default: true,
                },
            ]
        );
    }

    #[test]
    fn an_install_section_names_the_default() {
        let profiles = profiles_in(
            Path::new(ROOT),
            "[Install4F96D1932A9F858E]\nDefault=b.default-release\nLocked=1\n\n[Profile1]\nName=default\nIsRelative=1\nPath=a.default\nDefault=1\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=b.default-release\n",
        );
        let defaults: Vec<&str> = profiles
            .iter()
            .filter(|p| p.default)
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(defaults, vec!["default-release"]);
    }

    #[test]
    fn an_install_default_may_be_absolute() {
        let profiles = profiles_in(
            Path::new(ROOT),
            "[InstallABC]\nDefault=/mnt/tb/profile\n\n[Profile0]\nName=elsewhere\nIsRelative=0\nPath=/mnt/tb/profile\n",
        );
        assert!(profiles[0].default);
    }

    #[test]
    fn odd_sections_and_lines_are_passed_over() {
        let profiles = profiles_in(
            Path::new(ROOT),
            "; a comment\nstray=before any section\n[ProfileX]\nPath=no\n[Profile0]\nName=\nIsRelative=1\nPath=p.default\n[Profile1]\nName=no path\n[Profile2]\nIsRelative=0\nPath=relative/but/says/absolute\n[BackgroundTasksProfiles]\nMozillaBackgroundTask=x\n",
        );
        assert_eq!(profiles.len(), 1);
        // A profile without a name is called after its folder.
        assert_eq!(profiles[0].name, "p.default");
        assert!(!profiles[0].default);
    }

    #[test]
    fn windows_line_ends_and_spaces() {
        let profiles = profiles_in(
            Path::new(ROOT),
            "[Profile0]\r\nName = spaced \r\nIsRelative=1\r\nPath=s.default\r\nDefault=1\r\n",
        );
        assert_eq!(profiles[0].name, "spaced");
        assert_eq!(profiles[0].dir, Path::new(ROOT).join("s.default"));
        assert!(profiles[0].default);
    }
}
