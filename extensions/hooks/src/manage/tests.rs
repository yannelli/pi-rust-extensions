use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;
use crate::config::{LoadInput, Loaded, load};
use crate::paths::Paths;

struct Fixture {
    _dir: tempfile::TempDir,
    paths: Paths,
    agent: PathBuf,
    cwd: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_path_buf();
    let cwd = base.join("repo");
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    Fixture {
        paths: Paths {
            claude_dir: base.join("claude"),
            codex_home: base.join("codex"),
            codex_system: None,
            claude_policy: None,
            codex_mdm: None,
            temp_dir: base.join("tmp"),
            agent_dir: None,
        },
        agent: base.join("agent"),
        cwd,
        _dir: dir,
    }
}

fn load_with(f: &Fixture) -> Loaded {
    load(&LoadInput {
        paths: &f.paths,
        agent_dir: &f.agent,
        cwd: &f.cwd,
        project_trusted: true,
    })
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn find<'a>(loaded: &'a Loaded, command: &str) -> &'a HookDef {
    loaded
        .hooks
        .iter()
        .find(|h| h.summary() == command)
        .unwrap_or_else(|| panic!("no hook {command}"))
}

const CLAUDE: &str = r#"{
    "model": "opus",
    "hooks": {
        "PreToolUse": [
            {
                "matcher": "Bash",
                "hooks": [
                    { "type": "command", "command": "a" },
                    { "type": "command", "command": "b" }
                ]
            }
        ],
        "Stop": [ { "hooks": [ { "type": "command", "command": "c" } ] } ]
    },
    "permissions": { "allow": [] }
}
"#;

#[test]
fn disable_and_enable_persist_per_scope() {
    let f = fixture();
    write(&f.paths.claude_dir.join("settings.json"), CLAUDE);
    write(&f.cwd.join(".claude/settings.local.json"), CLAUDE);
    let loaded = load_with(&f);
    let global = loaded
        .hooks
        .iter()
        .find(|h| h.scope == Scope::Global && h.summary() == "a")
        .unwrap();
    let project = loaded
        .hooks
        .iter()
        .find(|h| h.scope == Scope::Project && h.summary() == "a")
        .unwrap();
    set_disabled(&loaded.global_file, global, true, true).unwrap();
    set_disabled(&loaded.project_file, project, true, true).unwrap();
    assert_eq!(read(&f.paths.claude_dir.join("settings.json")), CLAUDE);
    let reloaded = load_with(&f);
    assert!(reloaded.options.disabled.contains(&global.hash));
    assert!(reloaded.options.disabled.contains(&project.hash));
    let global_doc: Value = serde_json::from_str(&read(&loaded.global_file)).unwrap();
    assert_eq!(global_doc, json!({ "disabled": [global.hash] }));
    let project_doc: Value = serde_json::from_str(&read(&loaded.project_file)).unwrap();
    assert_eq!(project_doc, json!({ "disabled": [project.hash] }));
    assert_eq!(reloaded.hooks.len(), loaded.hooks.len());

    set_disabled(&loaded.project_file, project, false, true).unwrap();
    let reloaded = load_with(&f);
    assert!(reloaded.options.disabled.contains(&global.hash));
    assert!(!reloaded.options.disabled.contains(&project.hash));
    assert_eq!(read(&loaded.project_file), "{}\n");
}

#[test]
fn disable_keeps_other_keys_in_pi_file() {
    let f = fixture();
    let pi = f.agent.join("ext/hooks.json");
    write(&pi, "{\n\t\"trustAll\": true,\n\t\"x\": [1]\n}");
    write(&f.paths.claude_dir.join("settings.json"), CLAUDE);
    let loaded = load_with(&f);
    let def = find(&loaded, "c");
    set_disabled(&pi, def, true, true).unwrap();
    let text = read(&pi);
    assert!(
        text.starts_with("{\n\t\"trustAll\": true,\n\t\"x\": ["),
        "{text}"
    );
    assert!(!text.ends_with('\n'));
    assert!(load_with(&f).options.disabled.contains(&def.hash));
}

#[test]
fn remove_edits_claude_settings_and_keeps_the_rest() {
    let f = fixture();
    let file = f.cwd.join(".claude/settings.json");
    write(&file, CLAUDE);
    let loaded = load_with(&f);
    remove(find(&loaded, "a"), true).unwrap();
    let text = read(&file);
    assert!(
        text.starts_with("{\n    \"model\": \"opus\",\n    \"hooks\""),
        "{text}"
    );
    let doc: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["permissions"], json!({ "allow": [] }));
    assert_eq!(
        doc["hooks"]["PreToolUse"][0]["hooks"],
        json!([{ "type": "command", "command": "b" }])
    );
    let loaded = load_with(&f);
    remove(find(&loaded, "b"), true).unwrap();
    remove(find(&load_with(&f), "c"), true).unwrap();
    let doc: Value = serde_json::from_str(&read(&file)).unwrap();
    assert_eq!(
        doc,
        json!({ "model": "opus", "hooks": {}, "permissions": { "allow": [] } })
    );
    assert!(load_with(&f).hooks.is_empty());
}

#[test]
fn remove_edits_codex_toml_and_keeps_formatting() {
    let f = fixture();
    let file = f.paths.codex_home.join("config.toml");
    let text = r#"# my config
model = "o3"

[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "keep"

[[hooks.PreToolUse.hooks]]
type = "command"   # inline note
command = "drop"
"#;
    write(&file, text);
    let loaded = load_with(&f);
    remove(find(&loaded, "drop"), true).unwrap();
    let after = read(&file);
    assert!(
        after.starts_with("# my config\nmodel = \"o3\"\n"),
        "{after}"
    );
    assert!(after.contains("command = \"keep\""), "{after}");
    assert!(!after.contains("drop"), "{after}");
    remove(find(&load_with(&f), "keep"), true).unwrap();
    assert!(!read(&file).contains("PreToolUse"));
    assert!(load_with(&f).hooks.is_empty());
}

#[test]
fn add_writes_the_pi_file_of_the_scope() {
    let f = fixture();
    let loaded = load_with(&f);
    let global = add(&loaded.global_file, Scope::Global, true, "Stop", None, "g").unwrap();
    let project = add(
        &loaded.project_file,
        Scope::Project,
        true,
        "PreToolUse",
        Some("Bash"),
        "p",
    )
    .unwrap();
    let doc: Value = serde_json::from_str(&read(&loaded.project_file)).unwrap();
    assert_eq!(
        doc,
        json!({ "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "p" }] }] } })
    );
    let reloaded = load_with(&f);
    let hashes: Vec<&str> = reloaded.hooks.iter().map(|h| h.hash.as_str()).collect();
    assert_eq!(hashes, [global.hash.as_str(), project.hash.as_str()]);
    assert_eq!(find(&reloaded, "p").scope, Scope::Project);
    assert!(add(&loaded.global_file, Scope::Global, true, "Stop", None, " ").is_err());
}

#[test]
fn untrusted_project_is_refused() {
    let f = fixture();
    write(&f.cwd.join(".claude/settings.json"), CLAUDE);
    let loaded = load_with(&f);
    let def = find(&loaded, "a");
    let err = add(
        &loaded.project_file,
        Scope::Project,
        false,
        "Stop",
        None,
        "x",
    )
    .unwrap_err();
    assert_eq!(err.to_string(), PROJECT_UNTRUSTED);
    assert!(set_disabled(&loaded.project_file, def, true, false).is_err());
    assert!(remove(def, false).is_err());
    assert!(!loaded.project_file.exists());
    assert_eq!(read(&f.cwd.join(".claude/settings.json")), CLAUDE);
    let global = add(&loaded.global_file, Scope::Global, false, "Stop", None, "x");
    assert!(global.is_ok());
}

#[test]
fn unreadable_files_are_not_overwritten() {
    let f = fixture();
    let pi = f.agent.join("ext/hooks.json");
    write(&pi, "{ not json");
    assert!(add(&pi, Scope::Global, true, "Stop", None, "x").is_err());
    assert_eq!(read(&pi), "{ not json");
}

#[test]
fn hooks_without_a_config_file_are_not_removed() {
    let f = fixture();
    write(&f.paths.claude_dir.join("settings.json"), CLAUDE);
    let loaded = load_with(&f);
    let mut cloud = find(&loaded, "c").clone();
    cloud.file = f
        .paths
        .codex_home
        .join("cloud-config-bundle-cache.json#config_toml/f");
    let err = remove(&cloud, true).unwrap_err().to_string();
    assert!(err.contains("is not a file pi can edit"), "{err}");
}

#[test]
fn plugin_hooks_disable_under_a_stable_key_and_replace_old_entries() {
    let f = fixture();
    let root = f.paths.codex_home.join("plugins/cache/m/p/1.0.0");
    write(&root.join(".codex-plugin/plugin.json"), r#"{"name":"p"}"#);
    let hooks =
        json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "p" }] }] } });
    write(&root.join("hooks/hooks.json"), &hooks.to_string());
    let config = "[plugins.\"p@m\"]\nenabled = true\n";
    write(&f.paths.codex_home.join("config.toml"), config);
    let def = find(&load_with(&f), "p").clone();
    let pi = f.agent.join("ext/hooks.json");
    write(&pi, &json!({ "disabled": [def.hash] }).to_string());
    set_disabled(&pi, &def, true, true).unwrap();
    let doc: Value = serde_json::from_str(&read(&pi)).unwrap();
    assert_eq!(doc["disabled"], json!([def.disabled_key()]));
    write(&root.join("hooks/run.sh"), "echo changed");
    let changed = find(&load_with(&f), "p").clone();
    assert_ne!(changed.hash, def.hash);
    assert_eq!(changed.disabled_key(), def.disabled_key());
    write(&pi, &json!({ "disabled": [def.hash] }).to_string());
    set_disabled(&pi, &def, false, true).unwrap();
    assert!(!read(&pi).contains("disabled"));
}
