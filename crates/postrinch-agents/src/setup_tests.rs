use super::*;

const KEY: &str = "postrinch_agent_abc_secret";

fn bridge() -> PathBuf {
    PathBuf::from("/opt/postrinch/postrinch-mcp")
}

#[test]
fn entry_names_are_slugs() {
    assert_eq!(entry_name("Claude Code"), "postrinch-claude-code");
    assert_eq!(
        entry_name("  Research: notebook #2 "),
        "postrinch-research-notebook-2"
    );
    assert_eq!(entry_name("—"), "postrinch");
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
    App::ClaudeCode
        .add_at(&path, "postrinch-claude-code", &bridge(), KEY)
        .unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    let server = &doc["mcpServers"]["postrinch-claude-code"];
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

    assert!(
        App::ClaudeCode
            .remove_at(&path, "postrinch-claude-code")
            .unwrap()
    );
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(doc["mcpServers"].get("postrinch-claude-code").is_none());
    assert_eq!(doc["mcpServers"]["other"]["command"], "x");
    assert!(
        !App::ClaudeCode
            .remove_at(&path, "postrinch-claude-code")
            .unwrap()
    );
}

#[test]
fn a_new_file_and_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cursor").join("mcp.json");
    assert!(!App::Cursor.remove_at(&path, "postrinch-x").unwrap());
    App::Cursor
        .add_at(&path, "postrinch-x", &bridge(), KEY)
        .unwrap();
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["mcpServers"]["postrinch-x"]["env"][KEY_ENV], KEY);
    assert!(doc["mcpServers"]["postrinch-x"].get("type").is_none());
    assert!(!dir.path().join("cursor/mcp.json.postrinch-backup").exists());
}

#[test]
fn a_file_that_does_not_read_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("broken.json", "{ not json"),
        ("list.json", r#"{"mcpServers": []}"#),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let err = App::ClaudeDesktop.add_at(&path, "postrinch-x", &bridge(), KEY);
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
    App::Codex
        .add_at(&path, "postrinch-codex", &bridge(), KEY)
        .unwrap();
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

    assert!(App::Codex.remove_at(&path, "postrinch-codex").unwrap());
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
