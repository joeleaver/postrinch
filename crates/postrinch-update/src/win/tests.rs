//! The Windows install's writes, against a scratch registry key and Start
//! menu (`scratch`): what `--install` adds, that a second run adds nothing,
//! and that the uninstall takes away only what the install added.

use windows_registry::Type;

use super::path::PATH_ADDED;
use super::registration::{OWN, SETUP};
use super::scratch::Scratch;
use super::uninstall::{remove_files, remove_programs};
use super::*;

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// An install folder with the window in it.
fn install_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("postrinch.exe"), "the window").unwrap();
    dir
}

fn entry(s: &Scratch, name: &str) -> Option<windows_registry::Key> {
    s.user().open(format!(r"{UNINSTALL}\{name}")).ok()
}

#[test]
fn installing_twice_leaves_one_of_each() {
    let s = Scratch::new();
    let dir = install_folder();
    let first = integrate(&s.places, dir.path(), "1.2.3").unwrap();
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].starts_with("Added "), "{first:?}");
    let again = integrate(&s.places, dir.path(), "1.2.3").unwrap();
    assert!(again.is_empty(), "{again:?}");

    let lnks: Vec<_> = std::fs::read_dir(&s.places.programs)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(lnks, [SHORTCUT]);
    let exe = dir.path().join("postrinch.exe");
    let (target, icon, index) = shortcut::read(&s.places.programs.join(SHORTCUT)).unwrap();
    // The temporary folder may be named short (`RUNNER~1`) on one side.
    let same = |a: &std::path::Path| a.canonicalize().unwrap() == exe.canonicalize().unwrap();
    assert!(same(&target), "{}", target.display());
    assert!(same(&icon), "{}", icon.display());
    assert_eq!(index, 0);

    let dir_text = dir.path().display().to_string();
    assert_eq!(s.path().as_deref(), Some(dir_text.as_str()));
    let env = s.user().open("Environment").unwrap();
    assert_eq!(env.get_type("Path").unwrap(), Type::ExpandString);

    let own = entry(&s, OWN).expect("an Apps & features entry");
    assert_eq!(own.get_string("DisplayVersion").unwrap(), "1.2.3");
    assert_eq!(
        own.get_string("UninstallString").unwrap(),
        format!("\"{}\" --uninstall", exe.display())
    );
    assert!(entry(&s, SETUP).is_none());
}

#[test]
fn the_uninstall_takes_away_what_the_install_added() {
    let s = Scratch::new();
    let env = s.user().create("Environment").unwrap();
    env.set_string("Path", r"C:\Other").unwrap();
    let dir = install_folder();
    integrate(&s.places, dir.path(), "1.2.3").unwrap();
    assert_eq!(
        s.path().unwrap(),
        format!(r"C:\Other;{}", dir.path().display())
    );
    // The person's plain-text PATH stays plain text.
    assert_eq!(env.get_type("Path").unwrap(), Type::String);

    deintegrate(&s.places, dir.path()).unwrap();
    assert_eq!(s.path().unwrap(), r"C:\Other");
    assert!(!s.places.programs.join(SHORTCUT).exists());
    assert!(entry(&s, OWN).is_none());
    assert!(s.user().open(STATE_KEY).is_err());
    // Twice is harmless.
    deintegrate(&s.places, dir.path()).unwrap();
}

#[test]
fn a_path_entry_the_person_had_stays() {
    let s = Scratch::new();
    let dir = install_folder();
    let theirs = format!(
        r"C:\Other;{}\",
        dir.path().display().to_string().to_uppercase()
    );
    let env = s.user().create("Environment").unwrap();
    env.set_expand_string("Path", &theirs).unwrap();

    let notes = integrate(&s.places, dir.path(), "1.2.3").unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(s.path().unwrap(), theirs);
    let state = s.user().open(STATE_KEY);
    assert!(state.is_err() || state.unwrap().get_string(PATH_ADDED).is_err());

    deintegrate(&s.places, dir.path()).unwrap();
    assert_eq!(s.path().unwrap(), theirs);
}

#[test]
fn over_a_setup_install_its_entry_gets_the_version() {
    let s = Scratch::new();
    let setup = s.user().create(format!(r"{UNINSTALL}\{SETUP}")).unwrap();
    setup.set_string("DisplayVersion", "0.1.0").unwrap();
    let dir = install_folder();
    integrate(&s.places, dir.path(), "0.2.0").unwrap();
    assert_eq!(setup.get_string("DisplayVersion").unwrap(), "0.2.0");
    assert!(
        entry(&s, OWN).is_none(),
        "a second entry in Apps & features"
    );

    registration::record_version(s.user(), "0.3.0").unwrap();
    assert_eq!(setup.get_string("DisplayVersion").unwrap(), "0.3.0");
}

#[test]
fn an_update_records_its_version_only_where_an_entry_is() {
    let s = Scratch::new();
    registration::record_version(s.user(), "0.3.0").unwrap();
    assert!(entry(&s, SETUP).is_none());
    assert!(entry(&s, OWN).is_none());

    let dir = install_folder();
    integrate(&s.places, dir.path(), "0.2.0").unwrap();
    registration::record_version(s.user(), "0.3.0").unwrap();
    let own = entry(&s, OWN).unwrap();
    assert_eq!(own.get_string("DisplayVersion").unwrap(), "0.3.0");
}

#[test]
fn the_uninstall_removes_only_what_postrinch_put_in_the_folder() {
    let dir = install_folder();
    let path = |name: &str| dir.path().join(name);
    for name in [
        "postrinch-mcp.exe",
        "postrinch-cli.exe",
        crate::MARKER,
        "postrinch.old",
        "postrinch-cli.incoming",
        "notes.txt",
    ] {
        std::fs::write(path(name), "x").unwrap();
    }
    std::fs::create_dir(path(crate::STAGE)).unwrap();
    std::fs::write(path(crate::STAGE).join("ready.json"), "{}").unwrap();

    remove_programs(dir.path()).unwrap();
    assert!(path(crate::MARKER).exists(), "the marker goes last");
    remove_files(dir.path()).unwrap();
    let mut left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["notes.txt", "postrinch.exe"]);
}
