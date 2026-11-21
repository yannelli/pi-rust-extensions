use serde_json::json;

use super::*;
use crate::config::codex_plugin_bundle::bundle_unchanged;
use crate::config::options::{self, Options};
use crate::config::{LoadInput, load as load_config};
use crate::paths::Paths;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn hooks(command: &str) -> Value {
    json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": command }] }] } })
}

fn install(home: &Path, id: &str, version: &str) -> PathBuf {
    let (name, market) = id.rsplit_once('@').unwrap();
    let root = home
        .join("plugins/cache")
        .join(market)
        .join(name)
        .join(version);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn commands(defs: &[HookDef]) -> Vec<String> {
    defs.iter()
        .map(|d| format!("{}:{}", d.label(), d.summary()))
        .collect()
}

#[test]
fn enabled_plugins_load_their_default_hooks_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let root = install(home, "fmt@acme", "1.2.0");
    write(&root.join(".codex-plugin/plugin.json"), r#"{"name":"fmt"}"#);
    write(&root.join("hooks/hooks.json"), &hooks("fmt").to_string());
    let off = install(home, "off@acme", "1.0.0");
    write(&off.join(".codex-plugin/plugin.json"), r#"{"name":"off"}"#);
    write(&off.join("hooks/hooks.json"), &hooks("off").to_string());
    let user = json!({ "plugins": { "fmt@acme": { "enabled": true }, "off@acme": {}, "_default": { "enabled": false } } });
    let defs = load(home, &[&user], None, &HashMap::new());
    assert_eq!(commands(&defs), ["codex/global plugin fmt@acme:fmt"]);
    let plugin = defs[0].plugin.as_ref().unwrap();
    assert_eq!(plugin.root.as_deref(), Some(&*root.display().to_string()));
    let data = home.join("plugins/data/fmt-acme").display().to_string();
    assert_eq!(plugin.data.as_deref(), Some(&*data));
    assert_eq!(defs[0].scope, Scope::Global);
    let features = json!({ "features": { "plugins": false } });
    assert!(load(home, &[&user, &features], None, &HashMap::new()).is_empty());
    assert!(load(home, &[&user], Some(false), &HashMap::new()).is_empty());
}

#[test]
fn manifest_hooks_replace_the_default_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let root = install(home, "p@m", "local");
    write(
        &root.join("hooks/hooks.json"),
        &hooks("default").to_string(),
    );
    write(&root.join("hooks/extra.json"), &hooks("extra").to_string());
    let manifest = root.join(".claude-plugin/plugin.json");
    let user = json!({ "plugins": { "p@m": { "enabled": true } } });
    let run = || commands(&load(home, &[&user], None, &HashMap::new()));
    write(&manifest, r#"{"hooks":"./hooks/extra.json"}"#);
    assert_eq!(run(), ["codex/global plugin p@m:extra"]);
    write(&manifest, r#"{"hooks":"../outside.json"}"#);
    assert_eq!(run(), ["codex/global plugin p@m:default"]);
    let inline = json!({ "hooks": [hooks("one"), { "hooks": {} }, hooks("two")] });
    write(&manifest, &inline.to_string());
    assert_eq!(
        run(),
        ["codex/global plugin p@m:one", "codex/global plugin p@m:two"]
    );
    write(&manifest, r#"{"hooks":{"hooks":{},"extra":1}}"#);
    assert_eq!(
        run(),
        ["codex/global plugin p@m:default"],
        "not a HooksFile"
    );
    let agent = json!({ "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", "name": "p" });
    write(&root.join("plugin.json"), &agent.to_string());
    assert!(run().is_empty(), "Codex 0.171.0 skips Agent Plugins hooks");
}

#[test]
fn hook_state_keys_and_bundle_changes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let root = install(home, "p@m", "2.0.0");
    write(&root.join(".codex-plugin/plugin.json"), r#"{"name":"p"}"#);
    write(&root.join("hooks/hooks.json"), &hooks("x").to_string());
    write(&root.join("hooks/run.sh"), "echo 1");
    let user = json!({ "plugins": { "p@m": {} } });
    let first = load(home, &[&user], None, &HashMap::new());
    write(&root.join("hooks/run.sh"), "echo 2");
    let second = load(home, &[&user], None, &HashMap::new());
    assert_ne!(
        first[0].hash, second[0].hash,
        "a changed script needs new trust"
    );
    let states = HashMap::from([("p@m:hooks/hooks.json:stop:0:0".to_owned(), false)]);
    assert!(load(home, &[&user], None, &states).is_empty());
}

#[test]
fn only_hooks_on_codex_bundled_allowlist_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let cleanup = json!({ "hooks": {
        "Stop": [{ "hooks": [
            { "type": "mcp_tool", "server": "node_repl", "tool": "turn_ended", "input": { "a": 1 } },
            { "type": "command", "command": "x" }
        ] }],
        "PreToolUse": [{ "matcher": "Bash", "hooks": [
            { "type": "mcp_tool", "server": "node_repl", "tool": "turn_ended" }
        ] }]
    } });
    let mut users = Vec::new();
    for id in ["browser@openai-bundled", "p@m"] {
        let root = install(home, id, "1.0.0");
        write(&root.join(".codex-plugin/plugin.json"), r#"{"name":"x"}"#);
        write(&root.join("hooks/hooks.json"), &cleanup.to_string());
        users.push(json!({ "plugins": { id: {} } }));
    }
    let run = |user: &Value| commands(&load(home, &[user], None, &HashMap::new()));
    let bundled = "codex/global plugin browser@openai-bundled";
    assert_eq!(
        run(&users[0]),
        [
            format!("{bundled}:x"),
            format!("{bundled}:mcp: node_repl/turn_ended")
        ],
        "the cleanup hook without a matcher is skipped"
    );
    assert_eq!(
        run(&users[1]).len(),
        3,
        "other plugins keep node_repl hooks"
    );
}

#[test]
fn switches_and_managed_only_gate_plugin_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let paths = Paths {
        claude_dir: base.join("claude"),
        codex_home: base.join("codex"),
        codex_system: Some(base.join("etc")),
        claude_policy: None,
        codex_mdm: None,
        temp_dir: base.join("tmp"),
        agent_dir: None,
    };
    let root = install(&paths.codex_home, "p@m", "1.0.0");
    write(&root.join(".codex-plugin/plugin.json"), r#"{"name":"p"}"#);
    write(&root.join("hooks/hooks.json"), &hooks("plug").to_string());
    let config = "[plugins.\"p@m\"]\nenabled = true\n";
    write(&paths.codex_home.join("config.toml"), config);
    let agent = base.join("agent");
    let cwd = base.join("repo");
    std::fs::create_dir_all(&cwd).unwrap();
    let run = || {
        let input = LoadInput {
            paths: &paths,
            agent_dir: &agent,
            cwd: &cwd,
            project_trusted: true,
        };
        commands(&load_config(&input).hooks)
    };
    assert_eq!(run(), ["codex/global plugin p@m:plug"]);
    let requirements = "allow_managed_hooks_only = true\n";
    write(&base.join("etc/requirements.toml"), requirements);
    assert!(run().is_empty(), "allow_managed_hooks_only skips plugins");
    let off = json!({ "managed": { "codexRequirements": false }, "plugins": { "codex": false } });
    write(&agent.join("ext/hooks.json"), &off.to_string());
    assert!(run().is_empty(), "plugins/codex off");
    let on = json!({ "managed": { "codexRequirements": false } });
    write(&agent.join("ext/hooks.json"), &on.to_string());
    assert_eq!(
        run(),
        ["codex/global plugin p@m:plug"],
        "requirements.toml off"
    );
    assert!(Options::default().switch_on(options::CODEX_PLUGINS));
}

#[test]
fn project_layers_turn_plugins_on_and_off_as_in_codex() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let paths = Paths {
        claude_dir: base.join("claude"),
        codex_home: base.join("codex"),
        codex_system: None,
        claude_policy: None,
        codex_mdm: None,
        temp_dir: base.join("tmp"),
        agent_dir: None,
    };
    let root = install(&paths.codex_home, "p@m", "1.0.0");
    write(&root.join(".codex-plugin/plugin.json"), r#"{"name":"p"}"#);
    write(&root.join("hooks/hooks.json"), &hooks("plug").to_string());
    let cwd = base.join("repo");
    let agent = base.join("agent");
    let run = |user: bool, project: bool| {
        let table = |on: bool| format!("[plugins.\"p@m\"]\nenabled = {on}\n");
        write(&paths.codex_home.join("config.toml"), &table(user));
        write(&cwd.join(".codex/config.toml"), &table(project));
        let input = LoadInput {
            paths: &paths,
            agent_dir: &agent,
            cwd: &cwd,
            project_trusted: true,
        };
        commands(&load_config(&input).hooks)
    };
    assert_eq!(run(false, true), ["codex/global plugin p@m:plug"]);
    assert!(run(true, false).is_empty());
}

#[test]
fn changed_plugin_files_stop_loaded_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let root = install(home, "p@m", "1.0.0");
    write(&root.join(".codex-plugin/plugin.json"), r#"{"name":"p"}"#);
    write(&root.join("hooks/hooks.json"), &hooks("x").to_string());
    let user = json!({ "plugins": { "p@m": {} } });
    let defs = load(home, &[&user], None, &HashMap::new());
    assert!(bundle_unchanged(&defs[0], &mut HashMap::new()));
    write(&root.join("hooks/run.sh"), "echo changed");
    assert!(!bundle_unchanged(&defs[0], &mut HashMap::new()));
}
