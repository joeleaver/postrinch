use serde_json::{Value, json};

use super::*;
use crate::wire::KEY_ENV;

const BRIDGE: &str = "/opt/postrinch/postrinch-mcp";

/// A home with both Claudes, and Postrinch's agents file.
struct Home {
    dir: tempfile::TempDir,
    agents: Agents,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let agents = Agents::load(&dir.path().join("agents.json")).unwrap();
        Self { dir, agents }
    }

    fn code(&self) -> Place {
        Place {
            path: self.dir.path().join(".claude.json"),
            kind: Kind::ClaudeCode,
            installed: true,
        }
    }

    fn desktop(&self) -> Place {
        Place {
            path: self.dir.path().join("Claude/claude_desktop_config.json"),
            kind: Kind::ClaudeDesktop,
            installed: true,
        }
    }

    fn cursor(&self) -> Place {
        Place {
            path: self.dir.path().join(".cursor/mcp.json"),
            kind: Kind::Cursor,
            installed: true,
        }
    }

    fn claude(&self) -> Vec<Place> {
        vec![self.code(), self.desktop()]
    }

    fn connect(&mut self, app: App, places: Vec<Place>, name: &str) -> Connected {
        connect_in(
            &mut self.agents,
            Some((app, Ok(places))),
            name,
            Reach::Every,
            Path::new(BRIDGE),
        )
        .unwrap()
    }

    fn agent_names(&self) -> Vec<&str> {
        self.agents.list().iter().map(|a| a.name.as_str()).collect()
    }
}

fn servers(place: &Place) -> serde_json::Map<String, Value> {
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&place.path).unwrap()).unwrap();
    doc["mcpServers"].as_object().cloned().unwrap_or_default()
}

fn write(place: &Place, doc: &Value) {
    std::fs::create_dir_all(place.path.parent().unwrap()).unwrap();
    std::fs::write(&place.path, serde_json::to_string_pretty(doc).unwrap()).unwrap();
}

fn server(key: &str) -> Value {
    json!({ "command": BRIDGE, "env": { KEY_ENV: key } })
}

/// An agent as the old split choices left it: set up for Claude (the
/// desktop app or Claude Code, both read as Claude now).
fn old_agent(home: &mut Home, name: &str) -> (Agent, String) {
    let (agent, key) = home.agents.create(name, Reach::Every).unwrap();
    let setup = Setup {
        app: App::Claude,
        entry: entry_name(name),
    };
    home.agents.set_setup(&agent.id, Some(setup)).unwrap();
    (agent, key)
}

#[test]
fn claude_gets_one_entry_in_both_files() {
    let mut home = Home::new();
    write(
        &home.code(),
        &json!({ "mcpServers": { "other": { "command": "x" } } }),
    );
    let made = home.connect(App::Claude, home.claude(), "Claude");
    let Outcome::Added { files, replaced } = &made.outcome else {
        panic!("{:?}", made.outcome);
    };
    assert_eq!(files, &[home.code().path, home.desktop().path]);
    assert!(replaced.is_empty());
    assert_eq!(made.entry, "postrinch-claude");
    let (code, desktop) = (servers(&home.code()), servers(&home.desktop()));
    let (a, b) = (&code["postrinch-claude"], &desktop["postrinch-claude"]);
    assert_eq!(a["command"], BRIDGE);
    assert_eq!(a["command"], b["command"]);
    assert_eq!(a["env"], b["env"]);
    assert_eq!(a["env"][KEY_ENV], made.key.as_str());
    assert_eq!(a["type"], "stdio");
    assert_eq!(code["other"]["command"], "x", "other servers stay");
    assert_eq!((code.len(), desktop.len()), (2, 1));
    let setup = made.agent.setup.as_ref().unwrap();
    assert_eq!(
        (setup.app, setup.entry.as_str()),
        (App::Claude, "postrinch-claude")
    );
}

#[test]
fn only_the_claude_that_is_here_is_written() {
    let mut home = Home::new();
    let made = home.connect(App::Claude, vec![home.desktop()], "Claude");
    assert!(matches!(made.outcome, Outcome::Added { .. }));
    assert!(home.desktop().path.exists());
    assert!(!home.code().path.exists());

    let none = home.connect(App::Claude, Vec::new(), "Claude");
    let Outcome::Trouble { error, replaced } = none.outcome else {
        panic!("no Claude here");
    };
    assert!(matches!(error, SetupError::NotHere("Claude")));
    assert!(replaced.is_empty());
    assert_eq!(none.agent.name, "Claude 2", "nothing replaced: a free name");
    assert!(none.agent.setup.is_none());
}

#[test]
fn removing_takes_every_copy_out() {
    let mut home = Home::new();
    let made = home.connect(App::Claude, home.claude(), "Claude");
    // A copy pasted by hand under another name, in a project.
    let mut doc: Value =
        serde_json::from_str(&std::fs::read_to_string(&home.code().path).unwrap()).unwrap();
    doc["projects"] = json!({ "/work": { "mcpServers": { "mail": server(&made.key) } } });
    write(&home.code(), &doc);
    let (other, other_key) = home.agents.create("Other", Reach::Every).unwrap();
    write(
        &home.cursor(),
        &json!({ "mcpServers": { "o": server(&other_key) } }),
    );

    let mut places = home.claude();
    places.push(home.cursor());
    let done = disconnect_in(&made.agent, &places);
    assert!(done.trouble.is_empty());
    assert_eq!(done.files, [home.code().path, home.desktop().path]);
    assert!(servers(&home.code()).is_empty());
    assert!(servers(&home.desktop()).is_empty());
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(&home.code().path).unwrap()).unwrap();
    assert_eq!(doc["projects"]["/work"]["mcpServers"], json!({}));
    assert!(
        servers(&home.cursor()).contains_key("o"),
        "another agent's stays"
    );
    assert!(disconnect_in(&other, &[home.code()]).files.is_empty());
}

#[test]
fn connecting_again_replaces_and_never_piles_up() {
    let mut home = Home::new();
    let first = home.connect(App::Claude, home.claude(), "Claude");
    // The dialog proposes the same name: the agent it replaces frees it.
    let replacing: Vec<String> = would_replace_in(&home.agents, App::Claude, &home.claude())
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(replacing, std::slice::from_ref(&first.agent.id));
    assert_eq!(
        home.agents.free_name_ignoring("Claude", &replacing),
        "Claude"
    );

    let second = home.connect(App::Claude, home.claude(), "Claude");
    let Outcome::Added { replaced, .. } = &second.outcome else {
        panic!("{:?}", second.outcome);
    };
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].id, first.agent.id);
    assert_eq!(home.agent_names(), ["Claude"]);
    assert!(
        home.agents.find_by_key(&first.key).is_none(),
        "old key revoked"
    );
    assert_eq!(second.agent.replaces, std::slice::from_ref(&first.agent.id));
    assert_eq!(
        home.agents.get(&first.agent.id).unwrap().id,
        second.agent.id
    );
    for place in home.claude() {
        let servers = servers(&place);
        assert_eq!(servers.len(), 1, "{}", place.path.display());
        assert_eq!(
            servers["postrinch-claude"]["env"][KEY_ENV],
            second.key.as_str()
        );
    }

    let third = home.connect(App::Claude, home.claude(), "Claude");
    assert_eq!(home.agent_names(), ["Claude"]);
    let mut chain = third.agent.replaces.clone();
    chain.sort();
    let mut expected = vec![first.agent.id, second.agent.id];
    expected.sort();
    assert_eq!(chain, expected, "rules that asked any earlier one move");
}

#[test]
fn the_old_split_entries_are_cleaned_up() {
    let mut home = Home::new();
    let (desktop, desktop_key) = old_agent(&mut home, "Claude Desktop");
    let (desktop2, desktop2_key) = old_agent(&mut home, "Claude Desktop 2");
    let (code, code_key) = old_agent(&mut home, "Claude Code");
    let (cursor, cursor_key) = home.agents.create("Cursor", Reach::Every).unwrap();
    home.agents
        .set_setup(
            &cursor.id,
            Some(Setup {
                app: App::Cursor,
                entry: "postrinch-cursor".into(),
            }),
        )
        .unwrap();
    let pimble = json!({ "command": "/usr/bin/pimble-mcp" });
    write(
        &home.desktop(),
        &json!({ "mcpServers": {
            "postrinch-claude-desktop": server(&desktop_key),
            "postrinch-claude-desktop-2": server(&desktop2_key),
            "pimble": pimble,
        }}),
    );
    write(
        &home.code(),
        &json!({
            "mcpServers": {
                "postrinch-claude-code": server(&code_key),
                // A stale entry under the new name, its agent long gone.
                "postrinch-claude": server("postrinch_agent_gone_x"),
            },
            "projects": { "/work": { "mcpServers": { "mail": server(&cursor_key) } } },
        }),
    );

    let made = home.connect(App::Claude, home.claude(), "Claude");
    let Outcome::Added { replaced, .. } = &made.outcome else {
        panic!("{:?}", made.outcome);
    };
    let mut gone: Vec<&str> = replaced.iter().map(|a| a.name.as_str()).collect();
    gone.sort();
    assert_eq!(gone, ["Claude Code", "Claude Desktop", "Claude Desktop 2"]);
    assert_eq!(
        home.agent_names(),
        ["Cursor", "Claude"],
        "Cursor's agent stays"
    );
    for old in [&desktop, &desktop2, &code] {
        assert_eq!(home.agents.get(&old.id).unwrap().id, made.agent.id);
    }

    let desktop_servers = servers(&home.desktop());
    assert_eq!(
        desktop_servers.keys().collect::<Vec<_>>(),
        ["pimble", "postrinch-claude"]
    );
    let code_servers = servers(&home.code());
    assert_eq!(
        code_servers.keys().collect::<Vec<_>>(),
        ["postrinch-claude"]
    );
    assert_eq!(
        code_servers["postrinch-claude"]["env"][KEY_ENV],
        made.key.as_str()
    );
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(&home.code().path).unwrap()).unwrap();
    assert_eq!(
        doc["projects"]["/work"]["mcpServers"],
        json!({}),
        "a project copy would load twice too"
    );
}

/// What the person's machine was left as by hand: one entry under the
/// suffixed name in both files, its agent set up for the desktop app, and
/// Claude Code's agent left with no entry anywhere.
#[test]
fn a_hand_fixed_machine_is_cleaned_up() {
    let mut home = Home::new();
    let (_, _) = old_agent(&mut home, "Claude Desktop");
    home.agents.remove("Claude Desktop").unwrap();
    let (_, key) = old_agent(&mut home, "Claude Desktop 2");
    let (_, _) = old_agent(&mut home, "Claude Code");
    let entry = json!({ "mcpServers": { "postrinch-claude-desktop-2": server(&key) } });
    write(&home.code(), &entry);
    write(&home.desktop(), &entry);
    // Agents saved before the choices became one.
    let file = home.dir.path().join("agents.json");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        text.replacen("\"claude\"", "\"claude_desktop\"", 1)
            .replacen("\"claude\"", "\"claude_code\"", 1),
    )
    .unwrap();
    home.agents = Agents::load(&file).unwrap();

    let names: Vec<String> = would_replace_in(&home.agents, App::Claude, &home.claude())
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, ["Claude Desktop 2", "Claude Code"]);
    let made = home.connect(App::Claude, home.claude(), "Claude");
    assert!(matches!(made.outcome, Outcome::Added { .. }));
    assert_eq!(home.agent_names(), ["Claude"]);
    for place in home.claude() {
        assert_eq!(
            servers(&place).keys().collect::<Vec<_>>(),
            ["postrinch-claude"]
        );
    }
}

#[test]
fn an_agent_pasted_into_claude_by_hand_is_replaced_too() {
    let mut home = Home::new();
    let (by_hand, key) = home.agents.create("Mail", Reach::Every).unwrap();
    let (kept, _) = home.agents.create("Elsewhere", Reach::Every).unwrap();
    write(
        &home.desktop(),
        &json!({ "mcpServers": { "mail": server(&key) } }),
    );
    let made = home.connect(App::Claude, home.claude(), "Claude");
    let Outcome::Added { replaced, .. } = &made.outcome else {
        panic!("{:?}", made.outcome);
    };
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].id, by_hand.id);
    assert!(home.agents.list().iter().any(|a| a.id == kept.id));
}

#[test]
fn an_unreadable_claude_file_changes_nothing() {
    let mut home = Home::new();
    let (old, old_key) = old_agent(&mut home, "Claude");
    write(
        &home.code(),
        &json!({ "mcpServers": { "postrinch-claude": server(&old_key) } }),
    );
    std::fs::create_dir_all(home.desktop().path.parent().unwrap()).unwrap();
    std::fs::write(&home.desktop().path, "{ not json").unwrap();
    let before = std::fs::read_to_string(&home.code().path).unwrap();

    let made = home.connect(App::Claude, home.claude(), "Claude");
    let Outcome::Trouble { error, replaced } = &made.outcome else {
        panic!("{:?}", made.outcome);
    };
    assert!(matches!(error, SetupError::Unreadable(_)));
    assert!(replaced.is_empty());
    assert!(
        home.agents.find_by_key(&old_key).is_some(),
        "old agent kept"
    );
    assert_eq!(made.agent.name, "Claude 2");
    assert_eq!(std::fs::read_to_string(&home.code().path).unwrap(), before);
    assert!(home.agents.get(&old.id).is_some());
}

#[test]
fn cursor_and_codex_add_an_entry_each_time() {
    let mut home = Home::new();
    let first = home.connect(App::Cursor, vec![home.cursor()], "Cursor");
    let second = home.connect(App::Cursor, vec![home.cursor()], "Cursor 2");
    for made in [&first, &second] {
        let Outcome::Added { replaced, .. } = &made.outcome else {
            panic!("{:?}", made.outcome);
        };
        assert!(replaced.is_empty());
    }
    let servers = servers(&home.cursor());
    assert_eq!(
        servers.keys().collect::<Vec<_>>(),
        ["postrinch-cursor", "postrinch-cursor-2"]
    );
    assert_eq!(home.agent_names(), ["Cursor", "Cursor 2"]);

    let codex = Place {
        path: home.dir.path().join(".codex/config.toml"),
        kind: Kind::Codex,
        installed: true,
    };
    let made = home.connect(App::Codex, vec![codex.clone()], "Codex");
    assert!(matches!(made.outcome, Outcome::Added { .. }));
    let text = std::fs::read_to_string(&codex.path).unwrap();
    assert!(text.contains("[mcp_servers.postrinch-codex]"), "{text}");
    assert_eq!(
        disconnect_in(&made.agent, std::slice::from_ref(&codex)).files,
        std::slice::from_ref(&codex.path)
    );
    assert!(
        !std::fs::read_to_string(&codex.path)
            .unwrap()
            .contains("postrinch-codex")
    );
}

#[test]
fn by_hand_writes_nothing() {
    let mut home = Home::new();
    let made = connect_in(
        &mut home.agents,
        None,
        "Mine",
        Reach::Every,
        Path::new(BRIDGE),
    )
    .unwrap();
    assert!(matches!(made.outcome, Outcome::ByHand));
    assert_eq!(made.entry, "postrinch-mine");
    assert!(!home.code().path.exists() && !home.desktop().path.exists());
}

#[test]
fn rules_can_ask_an_agent_the_terminal_can_start() {
    let mut home = Home::new();
    let made = home.connect(App::Claude, home.claude(), "Claude");
    let asked = askable_in(&home.agents, &home.code());
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].id, made.agent.id);
    assert_eq!(asked[0].entry, "postrinch-claude");
    // With only the desktop app here, `claude -p` would not find it.
    let mut desktop_only = Home::new();
    desktop_only.connect(App::Claude, vec![desktop_only.desktop()], "Claude");
    assert!(askable_in(&desktop_only.agents, &desktop_only.code()).is_empty());
}
