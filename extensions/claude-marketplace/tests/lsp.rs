mod common;

use std::path::{Path, PathBuf};

use common::fixture::{Fixture, read_json, write, write_json};
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::state::State;
use pi_ext_claude_marketplace::{Scope, cli, install, market};
use serde_json::{Value, json};

/// A stdio LSP server that publishes one finding with its first argument and `TOKEN`.
const SERVER: &str = r#"
import json, os, sys
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if not line.strip():
            break
        name, value = line.decode().split(":", 1)
        if name.lower() == "content-length":
            length = int(value)
    return json.loads(sys.stdin.buffer.read(length))
def send(message):
    message["jsonrpc"] = "2.0"
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    msg = read()
    method = msg.get("method")
    if method in ("initialize", "shutdown"):
        send({"id": msg["id"], "result": {"capabilities": {}}})
    elif method == "textDocument/didOpen":
        doc = msg["params"]["textDocument"]
        text = "zz-lsp %s %s %s" % (sys.argv[1], os.environ.get("TOKEN"), doc["languageId"])
        diag = {"range": {"start": {"line": 0, "character": 0}}, "message": text}
        send({"method": "textDocument/publishDiagnostics", "params": {"uri": doc["uri"], "diagnostics": [diag]}})
    elif method == "exit":
        sys.exit(0)
"#;

fn plugin_zz(root: &Path) {
    write_json(
        &root.join(".claude-plugin/plugin.json"),
        &json!({ "name": "zz", "lspServers": "./lsp/extra.json",
            "userConfig": {
                "level": { "type": "string", "title": "L", "description": "L", "default": "high" },
                "token": { "type": "string", "title": "T", "description": "T", "sensitive": true, "default": "s3cret" }
            } }),
    );
    write_json(
        &root.join(".lsp.json"),
        &json!({
            "zls": {
                "command": "python3",
                "args": ["${CLAUDE_PLUGIN_ROOT}/server.py", "${user_config.level}", "${CLAUDE_PLUGIN_DATA}"],
                "extensionToLanguage": { ".zz": "zz" },
                "env": { "TOKEN": "${user_config.token}" },
                "initializationOptions": { "root": "${CLAUDE_PLUGIN_ROOT}" },
                "settings": { "zz": { "lint": true } },
                "workspaceFolder": "${CLAUDE_PROJECT_DIR}/src",
                "startupTimeout": 5000, "restartOnCrash": false, "maxRestarts": 2, "wat": 1
            },
            "broken": { "command": "x" }
        }),
    );
    write_json(
        &root.join("lsp/extra.json"),
        &json!({ "lspServers": { "yy": { "command": "yy-ls", "extensionToLanguage": { ".yy": "yy" } } } }),
    );
    write_json(
        &root.join("alt/lsp.json"),
        &json!({ "alt": { "command": "alt-ls", "extensionToLanguage": { ".alt": "alt" } } }),
    );
    write(&root.join("server.py"), SERVER);
}

fn setup() -> (Fixture, Dirs) {
    let fx = Fixture::new();
    let dir = fx.repo.parent().unwrap().join("langs");
    let catalog = json!({ "name": "langs", "owner": { "name": "L" }, "plugins": [
        { "name": "zz", "source": "./zz",
          "lspServers": { "entry": { "command": "e", "extensionToLanguage": { ".e": "e" } } } }
    ]});
    write_json(&dir.join(".claude-plugin/marketplace.json"), &catalog);
    plugin_zz(&dir.join("zz"));
    market::add(&fx.env(true), dir.to_str().unwrap(), Scope::User).unwrap();
    let dirs = Dirs::new(&fx.agent);
    (fx, dirs)
}

fn servers(dirs: &Dirs) -> Value {
    read_json(&dirs.lens_lsp_file())["servers"].clone()
}

fn keys(dirs: &Dirs) -> Vec<String> {
    let servers = servers(dirs);
    servers.as_object().unwrap().keys().cloned().collect()
}

fn set_plugin_config(fx: &Fixture, value: Value) {
    let file = fx.agent.join("ext/claude-marketplace.json");
    let mut config = read_json(&file);
    config["plugins"]["zz@langs"] = value;
    write_json(&file, &config);
}

fn root(fx: &Fixture) -> PathBuf {
    fx.repo.parent().unwrap().join("langs/zz")
}

#[test]
fn install_stages_servers_in_lens_with_substitution() {
    let (fx, dirs) = setup();
    let mine = json!({ "command": "mine-ls", "languages": ["go"] });
    let own = json!({ "command": "own" });
    let existing = json!({ "servers": { "mine": mine, "plugin_zz_yy": own } });
    write_json(&dirs.lens_lsp_file(), &existing);
    let env = fx.env(true);
    let text = install::install(&env, "zz@langs", Scope::User).unwrap();
    assert!(text.contains("1 LSP server(s)"), "{text}");
    assert!(
        text.contains("LSP server plugin_zz_yy already exists"),
        "{text}"
    );
    assert!(
        text.contains("lens extension runs the LSP servers"),
        "{text}"
    );
    for needle in [
        "LSP server broken (it needs extensionToLanguage",
        "LSP server zls restartOnCrash, LSP server zls maxRestarts (lens starts a new server",
        "LSP server zls wat (unknown field",
        "marketplace entry lspServers (ignored because the plugin has a plugin.json",
    ] {
        assert!(text.contains(needle), "{needle} missing: {text}");
    }
    let all = servers(&dirs);
    assert_eq!(all["mine"], mine);
    assert_eq!(all["plugin_zz_yy"], own);
    let zls = &all["plugin_zz_zls"];
    let root = root(&fx);
    let data = dirs.plugin_data("langs", "zz").unwrap();
    assert_eq!(zls["plugin"], "zz@langs");
    assert_eq!(zls["command"], "python3");
    let args = json!([root.join("server.py"), "high", data]);
    assert_eq!(zls["args"], args);
    assert_eq!(zls["env"]["TOKEN"], "s3cret");
    assert_eq!(zls["env"]["CLAUDE_PLUGIN_ROOT"], json!(root));
    assert_eq!(zls["env"]["CLAUDE_PLUGIN_OPTION_LEVEL"], "high");
    assert_eq!(
        zls["initializationOptions"]["root"],
        "${CLAUDE_PLUGIN_ROOT}"
    );
    assert_eq!(zls["settings"], json!({ "zz": { "lint": true } }));
    assert_eq!(zls["workspaceFolder"], "./src");
    assert_eq!(zls["startupTimeout"], 5000);
    for key in ["restartOnCrash", "maxRestarts", "wat"] {
        assert!(zls.get(key).is_none(), "{key} staged: {zls}");
    }
    let info = cli::info(&env, "zz@langs").unwrap();
    assert!(info.contains("LSP servers (lens, "), "{info}");
    assert!(info.contains("  plugin_zz_zls: python3 (.zz)"), "{info}");
    assert!(!info.contains("unsupported"), "{info}");
}

#[test]
fn disable_enable_reinstall_and_uninstall_restage_servers() {
    let (fx, dirs) = setup();
    let env = fx.env(true);
    let mine = json!({ "command": "mine-ls", "languages": ["go"] });
    write_json(
        &dirs.lens_lsp_file(),
        &json!({ "servers": { "mine": mine } }),
    );
    install::install(&env, "zz@langs", Scope::User).unwrap();
    let staged = ["mine", "plugin_zz_zls", "plugin_zz_yy"];
    assert_eq!(keys(&dirs), staged);
    let text = cli::run(&env, "disable zz@langs").unwrap();
    assert!(
        text.contains("lens extension drops its LSP servers"),
        "{text}"
    );
    assert_eq!(keys(&dirs), ["mine"]);
    let record = State::load(&dirs).unwrap().plugins["zz@langs"].clone();
    assert!(record.lsp_keys.is_empty());
    let info = cli::info(&env, "zz@langs").unwrap();
    assert!(info.contains("LSP servers (not staged;"), "{info}");
    let text = cli::run(&env, "enable zz@langs").unwrap();
    assert!(text.contains("2 LSP server(s)"), "{text}");
    assert_eq!(keys(&dirs), staged);
    cli::run(&env, "reinstall zz@langs").unwrap();
    cli::run(&env, "update zz@langs").unwrap();
    assert_eq!(keys(&dirs), staged);
    cli::run(&env, "uninstall zz@langs").unwrap();
    assert_eq!(keys(&dirs), ["mine"]);
}

#[test]
fn config_paths_env_and_strict_apply_to_servers() {
    let (fx, dirs) = setup();
    let env = fx.env(true);
    install::install(&env, "zz@langs", Scope::User).unwrap();
    let paths = json!({ "paths": { "lspServers": "./alt/lsp.json" }, "env": { "TEAM": "core" } });
    set_plugin_config(&fx, paths);
    cli::run(&env, "reinstall zz@langs").unwrap();
    assert_eq!(keys(&dirs), ["plugin_zz_zls", "plugin_zz_alt"]);
    assert_eq!(servers(&dirs)["plugin_zz_alt"]["env"]["TEAM"], "core");
    let extra = json!({ "extraPaths": { "lspServers": ["./alt/lsp.json"] } });
    set_plugin_config(&fx, extra);
    cli::run(&env, "reinstall zz@langs").unwrap();
    let all = ["plugin_zz_zls", "plugin_zz_yy", "plugin_zz_alt"];
    assert_eq!(keys(&dirs), all);
    set_plugin_config(&fx, json!({ "strict": false }));
    let err = install::install(&env, "zz@langs", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("conflicting manifests"),
        "{err:#}"
    );
    assert_eq!(
        keys(&dirs),
        all,
        "a failed install keeps the staged servers"
    );
}

#[test]
fn project_scope_needs_trust_and_resolves_the_project_dir() {
    let (fx, _) = setup();
    let err = install::install(&fx.env(false), "zz@langs", Scope::Project).unwrap_err();
    assert!(format!("{err:#}").contains("trusted project"), "{err:#}");
    let dirs = Dirs::new(&fx.project.join(".pi"));
    assert!(!dirs.lens_lsp_file().exists());
    install::install(&fx.env(true), "zz@langs", Scope::Project).unwrap();
    assert_eq!(
        dirs.lens_lsp_file(),
        fx.project.join(".pi/ext/lens-lsp.json")
    );
    let zls = &servers(&dirs)["plugin_zz_zls"];
    let src = format!("{}/src", fx.project.display());
    assert_eq!(zls["workspaceFolder"], src);
    assert_eq!(zls["env"]["CLAUDE_PROJECT_DIR"], json!(fx.project));
    assert!(!fx.agent.join("ext/lens-lsp.json").exists());
}

#[cfg(unix)]
#[test]
fn lens_runs_a_staged_plugin_server_in_pi() {
    use pi_ext_e2e::{Run, Turn, cargo_bin, pi_bin_or_skip};
    if pi_bin_or_skip().is_none() {
        return;
    }
    let (fx, dirs) = setup();
    install::install(&fx.env(true), "zz@langs", Scope::User).unwrap();
    let staged = std::fs::read_to_string(dirs.lens_lsp_file()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["lens"])
        .file(
            "agent/ext/lens.json",
            json!({ "autoDetect": false }).to_string(),
        )
        .file("agent/ext/lens-lsp.json", staged)
        .file("work/src/.keep", "")
        .turns([
            Turn::tool_call("write", json!({ "path": "a.zz", "content": "zz\n" })),
            Turn::tool_call("lens_diagnostics", json!({ "source": "session" })),
            Turn::text("done"),
        ])
        .prompt("Write a.zz and show Lens diagnostics.")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let text = output.tool_result_text("lens_diagnostics").unwrap();
    assert!(
        text.contains("zz-lsp high s3cret zz"),
        "{text}\n{}",
        output.stderr
    );
}
