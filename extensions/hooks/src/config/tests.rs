use std::path::Path;

use serde_json::json;

use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    paths: Paths,
    agent: PathBuf,
    cwd: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_path_buf();
    let paths = Paths {
        claude_dir: base.join("claude"),
        codex_home: base.join("codex"),
        temp_dir: base.join("tmp"),
        agent_dir: None,
    };
    let cwd = base.join("repo");
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    Fixture {
        _dir: dir,
        paths,
        agent: base.join("agent"),
        cwd,
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn hooks_json(command: &str) -> String {
    json!({ "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": command }] }] } })
        .to_string()
}

fn load_with(f: &Fixture, trusted: bool) -> Loaded {
    load(&LoadInput {
        paths: &f.paths,
        agent_dir: &f.agent,
        cwd: &f.cwd,
        project_trusted: trusted,
    })
}

fn commands(loaded: &Loaded) -> Vec<String> {
    loaded
        .hooks
        .iter()
        .map(|h| format!("{}:{}", h.label(), h.summary()))
        .collect()
}

fn populate(f: &Fixture) {
    write(&f.paths.claude_dir.join("settings.json"), &hooks_json("cg"));
    write(&f.cwd.join(".claude/settings.json"), &hooks_json("cp"));
    write(
        &f.cwd.join(".claude/settings.local.json"),
        &hooks_json("cl"),
    );
    write(&f.paths.codex_home.join("hooks.json"), &hooks_json("xg"));
    write(&f.cwd.join(".codex/hooks.json"), &hooks_json("xp"));
    write(&f.agent.join("ext/hooks.json"), &hooks_json("pg"));
    write(&f.cwd.join(".pi/ext/hooks.json"), &hooks_json("pp"));
}

#[test]
fn merges_all_sources_and_scopes() {
    let f = fixture();
    populate(&f);
    let loaded = load_with(&f, true);
    assert_eq!(
        commands(&loaded),
        [
            "claude/global:cg",
            "claude/project:cp",
            "claude/project:cl",
            "codex/global:xg",
            "codex/project:xp",
            "pi/global:pg",
            "pi/project:pp"
        ]
    );
    assert!(loaded.untrusted.is_empty());
}

#[test]
fn untrusted_project_skips_project_files() {
    let f = fixture();
    populate(&f);
    write(
        &f.cwd.join(".pi/ext/hooks.json"),
        r#"{"trustAll":true,"hooks":{}}"#,
    );
    let loaded = load_with(&f, false);
    assert_eq!(
        commands(&loaded),
        ["claude/global:cg", "codex/global:xg", "pi/global:pg"]
    );
    assert_eq!(loaded.untrusted.len(), 4);
    assert!(
        !loaded.options.trust_all,
        "untrusted project options are ignored"
    );
}

#[test]
fn scope_and_sources_options() {
    let f = fixture();
    populate(&f);
    let opts = r#"{"scope":"project","sources":{"codex":false},"hooks":{}}"#;
    write(&f.agent.join("ext/hooks.json"), opts);
    let loaded = load_with(&f, true);
    assert_eq!(
        commands(&loaded),
        ["claude/project:cp", "claude/project:cl", "pi/project:pp"]
    );
    write(
        &f.agent.join("ext/hooks.json"),
        r#"{"scope":"global","sources":{"claude":false,"pi":false}}"#,
    );
    assert_eq!(commands(&load_with(&f, true)), ["codex/global:xg"]);
}

#[test]
fn disable_all_hooks_and_codex_feature() {
    let f = fixture();
    populate(&f);
    write(
        &f.cwd.join(".claude/settings.local.json"),
        r#"{"disableAllHooks":true}"#,
    );
    write(
        &f.paths.codex_home.join("config.toml"),
        "[features]\nhooks = false\n",
    );
    assert_eq!(
        commands(&load_with(&f, true)),
        ["pi/global:pg", "pi/project:pp"]
    );
    write(
        &f.cwd.join(".codex/config.toml"),
        "[features]\nhooks = true\n",
    );
    assert_eq!(
        commands(&load_with(&f, true)),
        [
            "codex/global:xg",
            "codex/project:xp",
            "pi/global:pg",
            "pi/project:pp"
        ]
    );
}

#[test]
fn codex_toml_and_json_merge() {
    let f = fixture();
    write(&f.paths.codex_home.join("hooks.json"), &hooks_json("json"));
    let toml = "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"toml\"\n";
    write(&f.paths.codex_home.join("config.toml"), toml);
    let loaded = load_with(&f, true);
    assert_eq!(
        commands(&loaded),
        ["codex/global:json", "codex/global:toml"]
    );
    assert_eq!(loaded.hooks[1].event, "Stop");
    assert_eq!(loaded.hooks[1].matcher, None);
}

#[test]
fn pi_native_plugin_fields() {
    let f = fixture();
    let doc = json!({ "hooks": { "PreToolUse": [{
        "matcher": "Bash", "plugin": "fmt@acme", "pluginRoot": "/p", "pluginData": "/d", "extra": 1,
        "hooks": [{ "type": "command", "command": "/p/check.sh", "timeout": 30 }]
    }] } });
    write(&f.agent.join("ext/hooks.json"), &doc.to_string());
    let loaded = load_with(&f, true);
    let plugin = loaded.hooks[0].plugin.clone().unwrap();
    assert_eq!(plugin.id.as_deref(), Some("fmt@acme"));
    assert_eq!(plugin.root.as_deref(), Some("/p"));
    assert_eq!(plugin.data.as_deref(), Some("/d"));
    assert_eq!(loaded.hooks[0].label(), "pi/global plugin fmt@acme");
    assert_eq!(loaded.hooks[0].timeout().as_secs(), 30);
}
