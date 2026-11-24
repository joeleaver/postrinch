use serde_json::Value;

use super::*;
use crate::setup_file::SettingsFile;

const KEY: &str = "postrinch_agent_abc_secret";

fn bridge() -> PathBuf {
    PathBuf::from("/opt/postrinch/postrinch-mcp")
}

fn place(path: PathBuf, kind: Kind) -> Place {
    Place {
        path,
        kind,
        installed: true,
    }
}

fn add(place: &Place, entry: &str) -> Result<bool, SetupError> {
    let mut file = SettingsFile::read(place)?;
    file.insert(entry, &bridge(), KEY);
    file.write()
}

fn remove(place: &Place, entry: &str) -> Result<bool, SetupError> {
    let mut file = SettingsFile::read(place)?;
    file.drop_servers(|s| s.name == entry);
    file.write()
}

#[test]
fn entry_names_are_slugs() {
    assert_eq!(entry_name("Claude"), "postrinch-claude");
    assert_eq!(
        entry_name("  Research: notebook #2 "),
        "postrinch-research-notebook-2"
    );
    assert_eq!(entry_name("—"), "postrinch");
}

#[test]
fn the_bridge_is_known_wherever_it_is() {
    assert!(is_bridge("/opt/postrinch/postrinch-mcp"));
    assert!(is_bridge("postrinch-mcp"));
    assert!(is_bridge(r"C:\Program Files\Postrinch\postrinch-mcp.exe"));
    assert!(!is_bridge("/usr/bin/pimble-mcp"));
    assert!(!is_bridge("/opt/postrinch-mcp/other"));
}

#[test]
fn old_choices_read_as_claude() {
    for old in ["\"claude_desktop\"", "\"claude_code\"", "\"claude\""] {
        assert_eq!(serde_json::from_str::<App>(old).unwrap(), App::Claude);
    }
    assert_eq!(serde_json::to_string(&App::Claude).unwrap(), "\"claude\"");
}

#[test]
fn json_keeps_everything_else_and_its_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude.json");
    let before = r#"{
  "zeta": 1,
  "mcpServers": { "other": { "command": "x" } },
  "alpha": [true]
}"#;
    std::fs::write(&path, before).unwrap();
    let code = place(path.clone(), Kind::ClaudeCode);
    assert!(add(&code, "postrinch-claude").unwrap());

    let text = std::fs::read_to_string(&path).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    let server = &doc["mcpServers"]["postrinch-claude"];
    assert_eq!(server["command"], "/opt/postrinch/postrinch-mcp");
    assert_eq!(server["env"][KEY_ENV], KEY);
    assert_eq!(server["type"], "stdio");
    assert_eq!(doc["mcpServers"]["other"]["command"], "x");
    assert!(
        text.find("zeta").unwrap() < text.find("alpha").unwrap(),
        "order kept"
    );
    let backup = dir.path().join("claude.json.postrinch-backup");
    assert_eq!(std::fs::read_to_string(backup).unwrap(), before);

    assert!(remove(&code, "postrinch-claude").unwrap());
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(doc["mcpServers"].get("postrinch-claude").is_none());
    assert_eq!(doc["mcpServers"]["other"]["command"], "x");
    let unchanged = std::fs::read_to_string(&path).unwrap();
    assert!(
        !remove(&code, "postrinch-claude").unwrap(),
        "nothing to take out"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), unchanged);
}

#[test]
fn project_servers_in_claude_json_are_seen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".claude.json");
    let text = r#"{"projects": {"/work": {"mcpServers": {"p": {"command": "postrinch-mcp",
        "env": {"POSTRINCH_AGENT_KEY": "k"}}}}, "/empty": {}}}"#;
    std::fs::write(&path, text).unwrap();
    let file = SettingsFile::read(&place(path.clone(), Kind::ClaudeCode)).unwrap();
    let servers = file.servers();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].project.as_deref(), Some("/work"));
    assert_eq!(servers[0].key.as_deref(), Some("k"));
    // The desktop app has no projects: there they are just more settings.
    let desktop = SettingsFile::read(&place(path, Kind::ClaudeDesktop)).unwrap();
    assert!(desktop.servers().is_empty());
}

#[test]
fn the_desktop_file_has_no_type() {
    let dir = tempfile::tempdir().unwrap();
    let desktop = place(dir.path().join("desktop.json"), Kind::ClaudeDesktop);
    add(&desktop, "postrinch-claude").unwrap();
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(&desktop.path).unwrap()).unwrap();
    assert!(doc["mcpServers"]["postrinch-claude"].get("type").is_none());
}

#[test]
fn a_new_file_and_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cursor").join("mcp.json");
    let cursor = place(path.clone(), Kind::Cursor);
    assert!(!remove(&cursor, "postrinch-x").unwrap());
    assert!(!path.exists(), "nothing taken out, nothing written");
    add(&cursor, "postrinch-x").unwrap();
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["mcpServers"]["postrinch-x"]["env"][KEY_ENV], KEY);
    assert!(doc["mcpServers"]["postrinch-x"].get("type").is_none());
    assert!(!dir.path().join("cursor/mcp.json.postrinch-backup").exists());
}

#[test]
fn a_file_that_does_not_read_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    for (name, text, kind) in [
        ("broken.json", "{ not json", Kind::ClaudeDesktop),
        ("list.json", r#"{"mcpServers": []}"#, Kind::ClaudeDesktop),
        ("array.json", "[]", Kind::ClaudeCode),
        ("broken.toml", "mcp_servers = 3", Kind::Codex),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let err = add(&place(path.clone(), kind), "postrinch-x");
        assert!(matches!(err, Err(SetupError::Unreadable(_))), "{name}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "{name} untouched"
        );
    }
}

#[test]
fn toml_keeps_comments_and_other_servers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let before = "# my settings\nmodel = \"o4\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
    std::fs::write(&path, before).unwrap();
    let codex = place(path.clone(), Kind::Codex);
    add(&codex, "postrinch-codex").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# my settings\nmodel = \"o4\""), "{text}");
    let doc: toml_edit::DocumentMut = text.parse().unwrap();
    assert_eq!(doc["mcp_servers"]["other"]["command"].as_str(), Some("x"));
    let ours = &doc["mcp_servers"]["postrinch-codex"];
    assert_eq!(
        ours["command"].as_str(),
        Some("/opt/postrinch/postrinch-mcp")
    );
    assert_eq!(ours["env"][KEY_ENV].as_str(), Some(KEY));
    let file = SettingsFile::read(&codex).unwrap();
    let found = file.servers();
    let ours = found.iter().find(|s| s.name == "postrinch-codex").unwrap();
    assert_eq!(ours.key.as_deref(), Some(KEY));

    assert!(remove(&codex, "postrinch-codex").unwrap());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("postrinch-codex"), "{text}");
    assert!(text.contains("[mcp_servers.other]"));
}

#[test]
fn snippets_name_the_bridge_and_key() {
    let json = json_snippet("postrinch-x", &bridge(), KEY);
    assert!(json.contains("\"mcpServers\"") && json.contains(KEY));
    let toml = toml_snippet("postrinch-x", &bridge(), KEY);
    assert!(toml.contains("[mcp_servers.postrinch-x]"), "{toml}");
    assert!(command_snippet(&bridge(), KEY).contains(KEY_ENV));
}
