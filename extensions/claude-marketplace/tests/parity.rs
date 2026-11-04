mod common;

use std::path::{Path, PathBuf};

use common::TestHost;
use common::fixture::{Fixture, read, read_json, write, write_json};
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::state::State;
use pi_ext_claude_marketplace::{Scope, bin, cli, extension, install, market, styles};
use serde_json::{Value, json};

const HOOKS: &str = r#"{ "hooks": {
  "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command",
    "command": "${CLAUDE_PLUGIN_ROOT}/check.sh ${user_config.token}",
    "args": ["--token", "${user_config.token}"] } ] } ],
  "Stop": [ { "hooks": [ { "type": "command", "command": "echo stop" } ] } ] } }"#;

/// The `kit` plugin: one of each component, a plugin.json, and a strict marketplace entry.
fn plugin_kit(root: &Path) {
    write_json(
        &root.join(".claude-plugin/plugin.json"),
        &json!({
            "name": "kit", "version": "2.0.0",
            "commands": ["./cmds", { "deploy": { "content": "Deploy $ARGUMENTS[0] to $ARGUMENTS[1]",
                "description": "Deploy", "argumentHint": "<app> <env>" } }],
            "agents": "./roles",
            "mcpServers": { "api": { "command": "${CLAUDE_PLUGIN_ROOT}/api",
                "env": { "TOKEN": "${user_config.token}" } } },
            "userConfig": {
                "token": { "type": "string", "title": "Token", "description": "API token", "sensitive": true, "required": true },
                "region": { "type": "string", "title": "Region", "description": "Region", "default": "eu" }
            },
            "dependencies": ["base"],
            "experimental": { "monitors": [{ "name": "m", "command": "tail", "description": "d" }] }
        }),
    );
    write(
        &root.join("cmds/ship.md"),
        "---\narguments: [app, target]\nargument-hint: <app> <target>\n---\nShip $app to $target\n",
    );
    write(&root.join("commands/ignored.md"), "Ignored\n");
    write(
        &root.join("roles/review/audit.md"),
        "---\nname: audit\nmodel: sonnet\neffort: high\ntools: Read, WebSearch\n---\nAudit in ${user_config.region}\n",
    );
    write(
        &root.join("skills/notes/SKILL.md"),
        "---\nname: notes\n---\nRegion ${user_config.region}, token ${user_config.token}\n",
    );
    write(
        &root.join("output-styles/terse.md"),
        "---\nname: terse\ndescription: Short answers\n---\nAnswer in one line.\n",
    );
    write(&root.join("hooks/hooks.json"), HOOKS);
    write_json(
        &root.join(".mcp.json"),
        &json!({ "mcpServers": { "docs": { "url": "https://x/mcp" } } }),
    );
    write(&root.join("bin/kit-tool"), "#!/bin/sh\necho kit\n");
    write_json(&root.join("settings.json"), &json!({ "agent": "audit" }));
    write(&root.join("themes/dark.json"), "{}");
}

fn marketplace(dir: &Path) -> PathBuf {
    let catalog = json!({ "name": "kits", "owner": { "name": "K" }, "plugins": [
        { "name": "kit", "source": "./kit", "description": "Kit",
          "commands": "./extra",
          "mcpServers": { "ignored": { "command": "x" } },
          "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "echo entry" } ] } ] } },
        { "name": "base", "source": "./base" },
        { "name": "lsp", "source": "./lsp", "strict": false,
          "lspServers": { "go": { "command": "gopls", "extensionToLanguage": { ".go": "go" } } } },
        { "name": "clash", "source": "./base", "strict": false, "skills": ["./skills"] },
        { "name": "solo", "source": "./solo" }
    ]});
    write_json(&dir.join(".claude-plugin/marketplace.json"), &catalog);
    plugin_kit(&dir.join("kit"));
    write(&dir.join("kit/extra/more.md"), "More $ARGUMENTS\n");
    write_json(
        &dir.join("base/.claude-plugin/plugin.json"),
        &json!({ "name": "base" }),
    );
    write(&dir.join("base/commands/hi.md"), "Hi $1\n");
    write(&dir.join("lsp/README.md"), "LSP\n");
    write(
        &dir.join("solo/SKILL.md"),
        "---\ndescription: One skill\n---\nSolo\n",
    );
    dir.to_path_buf()
}

struct Kit {
    fx: Fixture,
    dirs: Dirs,
}

fn setup() -> Kit {
    let fx = Fixture::new();
    let dir = marketplace(&fx.repo.parent().unwrap().join("kits"));
    market::add(&fx.env(true), dir.to_str().unwrap(), Scope::User).unwrap();
    let dirs = Dirs::new(&fx.agent);
    Kit { fx, dirs }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn installs_every_component_with_strict_entry_rules() {
    let kit = setup();
    let env = kit.fx.env(true);
    let text = install::install(&env, "kit@kits", Scope::User).unwrap();
    for part in [
        "Installed kit@kits 2.0.0 [user]: 3 command(s), 1 skill(s), 1 agent(s), 2 hook group(s), 2 MCP server(s), 1 output style(s), 1 bin command(s), 2 user config option(s).",
        "Not supported in pi: monitors, themes, settings.",
        "the marketplace entry's mcpServers is ignored because the plugin has a plugin.json",
        "the default commands/ folder is ignored because the manifest sets commands",
        "Needs user config: token.",
        "Dependency: Installed base@kits [user]: 1 command(s)",
    ] {
        assert!(text.contains(part), "missing {part:?} in {text}");
    }
    let dirs = &kit.dirs;
    assert_eq!(
        names(&dirs.prompts()),
        ["base:hi.md", "kit:deploy.md", "kit:more.md", "kit:ship.md"]
    );
    let deploy = read(&dirs.prompts().join("kit:deploy.md"));
    assert_eq!(
        deploy,
        "---\ndescription: Deploy\nargument-hint: <app> <env>\n---\nDeploy $1 to $2"
    );
    let ship = read(&dirs.prompts().join("kit:ship.md"));
    assert!(ship.ends_with("---\nShip $1 to $2\n"), "{ship}");

    let agent = read(&dirs.agents().join("kit-review-audit.md"));
    assert!(agent.contains("name: kit-review-audit\n"), "{agent}");
    assert!(agent.contains("model: inherit\n"), "{agent}");
    assert!(agent.contains("thinking: high\n"), "{agent}");
    assert!(agent.contains("tools: read, web_search\n"), "{agent}");
    assert!(agent.contains("Audit in eu"), "default value: {agent}");

    let skill = read(&dirs.skills().join("kit-notes/SKILL.md"));
    assert!(
        skill.contains("Region eu, token ${user_config.token}"),
        "{skill}"
    );

    let hooks = read_json(&dirs.hooks_file());
    let stop = hooks["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(
        stop.len(),
        1,
        "the entry's Stop replaces the plugin's: {hooks}"
    );
    assert_eq!(stop[0]["hooks"][0]["command"], "echo entry");
    let pre = &hooks["hooks"]["PreToolUse"][0];
    assert_eq!(pre["pluginEnv"]["CLAUDE_PLUGIN_OPTION_REGION"], "eu");

    let mcp = read_json(&dirs.mcp_file());
    let servers = mcp["mcpServers"].as_object().unwrap();
    let mut keys: Vec<&String> = servers.keys().collect();
    keys.sort();
    assert_eq!(keys, ["plugin_kit_api", "plugin_kit_docs"]);
    let api = &servers["plugin_kit_api"];
    let root = kit.fx.repo.parent().unwrap().join("kits/kit");
    let data = dirs.plugin_data("kits", "kit").unwrap();
    assert_eq!(api["env"]["CLAUDE_PLUGIN_ROOT"], root.display().to_string());
    assert_eq!(api["env"]["CLAUDE_PLUGIN_DATA"], data.display().to_string());
    assert_eq!(api["env"]["TOKEN"], "${user_config.token}");
    assert!(
        servers["plugin_kit_docs"].get("env").is_none(),
        "http server"
    );

    let record = &State::load(dirs).unwrap().plugins["kit@kits"];
    assert_eq!(record.bin_names, ["kit-tool"]);
    assert_eq!(record.default_agent.as_deref(), Some("audit"));
    let info = cli::info(&env, "kit@kits").unwrap();
    for part in [
        "Commands: kit:deploy, kit:more, kit:ship",
        "Agents: kit-review-audit",
        "Output styles: kit:terse",
        "Bin (bash PATH): kit-tool",
        "  token = (missing, required)  -- Token",
        "  region = eu (default)  -- Region",
        "Not supported in pi: monitors, themes, settings",
        "Default agent (not applied): audit",
    ] {
        assert!(info.contains(part), "missing {part:?} in {info}");
    }
}

#[test]
fn user_config_is_stored_and_substituted() {
    let kit = setup();
    let env = kit.fx.env(true);
    install::install(&env, "kit@kits", Scope::User).unwrap();
    let text = cli::run(&env, "config kit@kits token=s3cret region=us").unwrap();
    assert!(
        text.starts_with("Configured kit@kits 2.0.0 [user]"),
        "{text}"
    );
    assert!(!text.contains("Needs user config"), "{text}");
    let file = kit.dirs.plugin_options("kits", "kit").unwrap();
    assert_eq!(
        read_json(&file),
        json!({ "token": "s3cret", "region": "us" })
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let mcp = read_json(&kit.dirs.mcp_file());
    assert_eq!(
        mcp["mcpServers"]["plugin_kit_api"]["env"]["TOKEN"],
        "s3cret"
    );
    let hooks = read_json(&kit.dirs.hooks_file());
    let pre = &hooks["hooks"]["PreToolUse"][0];
    assert_eq!(pre["pluginEnv"]["CLAUDE_PLUGIN_OPTION_TOKEN"], "s3cret");
    let handler = &pre["hooks"][0];
    assert_eq!(handler["args"], json!(["--token", "s3cret"]));
    assert!(
        handler["command"]
            .as_str()
            .unwrap()
            .ends_with("check.sh ${user_config.token}"),
        "no user config in a shell command: {handler}"
    );
    let skill = read(&kit.dirs.skills().join("kit-notes/SKILL.md"));
    assert!(
        skill.contains("Region us, token ${user_config.token}"),
        "{skill}"
    );

    let shown = cli::run(&env, "config kit@kits").unwrap();
    assert!(shown.contains("  token = (set)"), "{shown}");
    let err = cli::run(&env, "config kit@kits nope=1").unwrap_err();
    assert!(
        format!("{err:#}").contains("no user config option"),
        "{err:#}"
    );

    install::uninstall(&env, "kit@kits", None).unwrap();
    assert!(!file.exists());
}

#[test]
fn output_styles_select_and_force() {
    let kit = setup();
    let env = kit.fx.env(true);
    assert_eq!(styles::section(&env), None);
    install::install(&env, "kit@kits", Scope::User).unwrap();
    assert_eq!(styles::section(&env), None, "nothing selected");
    let list = cli::run(&env, "style").unwrap();
    assert!(list.contains("  kit:terse - Short answers"), "{list}");
    let text = cli::run(&env, "style kit:terse").unwrap();
    assert!(text.starts_with("Output style kit:terse [user]"), "{text}");
    assert_eq!(
        styles::section(&env).as_deref(),
        Some("# Output style: kit:terse\n\nAnswer in one line.")
    );
    assert!(cli::run(&env, "style nope").is_err());
    cli::run(&env, "style off").unwrap();
    assert_eq!(styles::section(&env), None);

    let style = kit
        .fx
        .repo
        .parent()
        .unwrap()
        .join("kits/kit/output-styles/loud.md");
    write(
        &style,
        "---\ndescription: Loud\nforce-for-plugin: true\n---\nShout.\n",
    );
    install::install(&env, "kit@kits", Scope::User).unwrap();
    let section = styles::section(&env).unwrap();
    assert_eq!(section, "# Output style: kit:loud\n\nShout.");
    install::uninstall(&env, "kit@kits", None).unwrap();
    assert_eq!(styles::section(&env), None);
}

#[test]
fn bin_dirs_join_path_for_commands_that_name_them() {
    let kit = setup();
    let env = kit.fx.env(true);
    install::install(&env, "kit@kits", Scope::User).unwrap();
    assert_eq!(bin::rewrite(&env, "ls -la"), None);
    if cfg!(windows) {
        return;
    }
    let dir = kit.fx.repo.parent().unwrap().join("kits/kit/bin");
    let rewritten = bin::rewrite(&env, "kit-tool --x | head").unwrap();
    assert_eq!(
        rewritten,
        format!(
            "export PATH=\"$PATH\":'{}'; kit-tool --x | head",
            dir.display()
        )
    );
    cli::run(&env, "disable kit@kits").unwrap();
    assert_eq!(bin::rewrite(&env, "kit-tool"), None);
}

#[test]
fn manifest_rules_lsp_conflicts_and_single_skill() {
    let kit = setup();
    let env = kit.fx.env(true);
    let text = install::install(&env, "lsp@kits", Scope::User).unwrap();
    assert!(text.contains("Not supported in pi: lspServers."), "{text}");
    let info = cli::info(&env, "lsp@kits").unwrap();
    assert!(
        info.contains("LSP servers (not run; add them to lens.json by hand): go: gopls"),
        "{info}"
    );

    let err = install::install(&env, "clash@kits", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("conflicting manifests"),
        "{err:#}"
    );
    let list = cli::run(&env, "list kits").unwrap();
    assert!(list.contains("[unavailable] clash"), "{list}");

    let text = install::install(&env, "solo@kits", Scope::User).unwrap();
    assert!(text.contains("1 skill(s)"), "{text}");
    let skill = read(&kit.dirs.skills().join("solo/SKILL.md"));
    assert!(
        skill.starts_with("---\ndescription: One skill\nname: solo\n---\n"),
        "{skill}"
    );
}

fn event(fx: &Fixture, name: &str, event: Value) -> Value {
    let ctx = json!({ "cwd": fx.project, "hasUI": false, "flags": {}, "isProjectTrusted": true });
    json!({ "name": name, "event": event, "ctx": ctx })
}

#[tokio::test]
async fn protocol_adds_style_section_and_bin_path() {
    let kit = setup();
    let env = kit.fx.env(true);
    install::install(&env, "kit@kits", Scope::User).unwrap();
    cli::run(&env, "style kit:terse").unwrap();
    let fx = &kit.fx;
    let mut host = TestHost::start(extension(fx.paths()));
    let init = host
        .initialize_in(fx.project.to_str().unwrap(), fx.agent.to_str().unwrap())
        .await;
    let events = init["events"].as_array().unwrap();
    assert!(events.contains(&json!("before_agent_start")), "{init}");
    assert!(events.contains(&json!("tool_call")), "{init}");

    let start = json!({ "type": "before_agent_start", "prompt": "hi", "systemPromptOptions": {} });
    host.request(1, "event", event(fx, "before_agent_start", start))
        .await;
    let reply = host.expect_response(1).await;
    let sections = &reply["result"]["patch"]["systemPromptOptions"]["sections"];
    assert_eq!(
        sections[styles::SECTION],
        "# Output style: kit:terse\n\nAnswer in one line."
    );

    let call = json!({ "type": "tool_call", "toolCallId": "c1", "toolName": "bash",
        "input": { "command": "kit-tool" } });
    host.request(2, "event", event(fx, "tool_call", call)).await;
    let reply = host.expect_response(2).await;
    let command = &reply["result"]["patch"]["input"]["command"];
    if cfg!(windows) {
        assert!(command.is_null(), "{reply}");
    } else {
        let text = command.as_str().unwrap();
        assert!(text.starts_with("export PATH=\"$PATH\":'"), "{text}");
        assert!(text.ends_with("/kits/kit/bin'; kit-tool"), "{text}");
    }

    let other = json!({ "type": "tool_call", "toolCallId": "c2", "toolName": "bash",
        "input": { "command": "ls" } });
    host.request(3, "event", event(fx, "tool_call", other))
        .await;
    let reply = host.expect_response(3).await;
    assert!(reply["result"].get("patch").is_none(), "{reply}");
    host.finish().await.unwrap();
}
