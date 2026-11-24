//! Connecting Claude when a file cannot be written, or the agents file
//! cannot be saved: no key is revoked unless the new one is in place.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::json;

use super::tests::{Home, old_agent, server, servers, write};
use super::*;
use crate::wire::KEY_ENV;

/// Make `dir` read-only until the guard goes. None when that does not stop
/// writing (as for root), so the test has nothing to show.
struct Locked<'a>(&'a Path);

fn lock(dir: &Path) -> Option<Locked<'_>> {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let locked = Locked(dir);
    let probe = dir.join("probe");
    if std::fs::write(&probe, "x").is_ok() {
        let _ = std::fs::remove_file(probe);
        return None;
    }
    Some(locked)
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
    }
}

fn parent(place: &Place) -> &Path {
    place.path.parent().unwrap()
}

/// Claude set up the old way: one agent, its entry in both files.
fn set_up_before(home: &mut Home) -> (Agent, String) {
    let (old, key) = old_agent(home, "Claude");
    let entry = json!({ "mcpServers": { "postrinch-claude": server(&key) } });
    write(&home.code(), &entry);
    write(&home.desktop(), &entry);
    (old, key)
}

#[test]
fn no_file_written_revokes_nothing() {
    let mut home = Home::new();
    let (old, key) = set_up_before(&mut home);
    let (code, desktop) = (home.code(), home.desktop());
    let before = std::fs::read_to_string(&code.path).unwrap();
    let Some(_a) = lock(parent(&code)) else {
        return;
    };
    let Some(_b) = lock(parent(&desktop)) else {
        return;
    };

    let made = home.connect(App::Claude, home.claude(), "Claude");
    let Outcome::Trouble { error, changes } = &made.outcome else {
        panic!("{:?}", made.outcome);
    };
    assert!(matches!(error, SetupError::Io { .. }), "{error}");
    assert!(changes.replaced.is_empty());
    assert_eq!(home.agents.find_by_key(&key).unwrap().id, old.id);
    assert_eq!(made.agent.name, "Claude 2", "the old one keeps its name");
    assert!(made.agent.setup.is_none(), "set up by hand, then");
    assert_eq!(std::fs::read_to_string(&code.path).unwrap(), before);
    assert_eq!(servers(&desktop)["postrinch-claude"]["env"][KEY_ENV], key);
}

#[test]
fn one_file_written_is_enough_to_replace() {
    let mut home = Home::new();
    let (old, key) = set_up_before(&mut home);
    let desktop = home.desktop();
    let Some(_locked) = lock(parent(&desktop)) else {
        return;
    };

    let made = home.connect(App::Claude, home.claude(), "Claude");
    let Outcome::Trouble { error, changes } = &made.outcome else {
        panic!("{:?}", made.outcome);
    };
    assert!(matches!(error, SetupError::Io { .. }), "{error}");
    assert_eq!(changes.replaced.len(), 1);
    assert_eq!(changes.replaced[0].id, old.id);
    assert!(home.agents.find_by_key(&key).is_none());
    assert_eq!(
        servers(&home.code())["postrinch-claude"]["env"][KEY_ENV],
        made.key.as_str()
    );
    assert!(made.agent.setup.is_some(), "removing it finds the entry");
}

#[test]
fn an_agents_file_that_cannot_be_saved_puts_the_files_back() {
    let mut home = Home::new();
    let (old, key) = old_agent(&mut home, "Claude");
    let code = home.code();
    write(
        &code,
        &json!({ "mcpServers": { "postrinch-claude": server(&key) } }),
    );
    let before = std::fs::read_to_string(&code.path).unwrap();
    std::fs::create_dir_all(parent(&home.desktop())).unwrap();
    let agents_dir = home.dir.path().join("postrinch");
    let Some(_locked) = lock(&agents_dir) else {
        return;
    };

    let (everywhere, claude) = (home.everywhere(), home.claude());
    let result = connect_in(
        &mut home.agents,
        Some((App::Claude, Ok(claude))),
        &everywhere,
        "Claude",
        Reach::Every,
        Path::new(super::tests::BRIDGE),
    );
    assert!(matches!(result, Err(RegistryError::Io { .. })));
    assert_eq!(std::fs::read_to_string(&code.path).unwrap(), before);
    assert!(!home.desktop().path.exists(), "a file made is taken away");
    assert_eq!(home.agents.find_by_key(&key).unwrap().id, old.id);
    assert_eq!(home.agents.list().len(), 1);
}
