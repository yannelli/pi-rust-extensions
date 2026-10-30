mod common;

use common::fixture::{Fixture, read, read_json, write_json};
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::state::State;
use pi_ext_claude_marketplace::{Scope, cli, install, market};
use serde_json::json;

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn install_and_uninstall_user_scope() {
    let fx = Fixture::new();
    let env = fx.env(true);
    let dirs = Dirs::new(&fx.agent);
    let user_group =
        json!({ "matcher": "Write", "hooks": [{ "type": "command", "command": "mine.sh" }] });
    write_json(
        &dirs.hooks_file(),
        &json!({ "sources": { "claude": true }, "trustAll": false,
        "hooks": { "PreToolUse": [user_group.clone()] } }),
    );
    write_json(
        &dirs.mcp_file(),
        &json!({ "mcpServers": { "mine": { "command": "x" } }, "other": 1 }),
    );

    let added = market::add(&env, fx.repo.to_str().unwrap(), Scope::User).unwrap();
    assert!(
        added.contains("Added marketplace acme (4 plugin(s))"),
        "{added}"
    );
    let text = install::install(&env, "fmt@acme", Scope::User).unwrap();
    assert!(
        text.contains("2 command(s), 2 skill(s), 1 agent(s), 1 hook group(s), 1 MCP server(s)"),
        "{text}"
    );
    assert!(text.contains("/reload"), "{text}");

    let root = fx.repo.join("plugins/fmt");
    assert_eq!(names(&dirs.prompts()), ["fmt:build:web.md", "fmt:hello.md"]);
    let web = read(&dirs.prompts().join("fmt:build:web.md"));
    assert_eq!(web, format!("Build with {}/web\n", root.display()));
    assert_eq!(names(&dirs.skills()), ["fmt-check", "fmt-style"]);
    let skill = read(&dirs.skills().join("fmt-check/SKILL.md"));
    let skill_dir = dirs.skills().join("fmt-check");
    assert!(
        skill.starts_with("---\nname: fmt-check\ndescription: Check code\n---\n"),
        "{skill}"
    );
    assert!(
        skill.contains(&format!("Run {}/run.sh", skill_dir.display())),
        "{skill}"
    );
    assert!(dirs.skills().join("fmt-check/ref.txt").is_file());
    assert!(read(&dirs.skills().join("fmt-style/SKILL.md")).contains("name: fmt-style"));

    let agent = read(&fx.agent.join("ext/subagents/agents/fmt-reviewer.md"));
    assert!(agent.contains("name: fmt-reviewer\n"), "{agent}");
    assert!(
        agent.contains("tools: bash, read, find, WebFetch\n"),
        "{agent}"
    );
    assert!(
        agent.contains("Review in ${CLAUDE_PROJECT_DIR}"),
        "user scope keeps the token: {agent}"
    );

    let hooks = read_json(&dirs.hooks_file());
    assert_eq!(hooks["sources"], json!({ "claude": true }));
    assert_eq!(hooks["trustAll"], json!(false));
    let groups = hooks["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0], user_group);
    assert_eq!(groups[1]["plugin"], "fmt@acme");
    assert_eq!(groups[1]["matcher"], "Bash");
    assert_eq!(groups[1]["pluginRoot"], root.display().to_string());
    let data = dirs.plugin_data("acme", "fmt");
    assert_eq!(groups[1]["pluginData"], data.display().to_string());
    let command = format!("{}/scripts/check.sh ${{UNKNOWN}}", root.display());
    assert_eq!(groups[1]["hooks"][0]["command"], command);

    let mcp = read_json(&dirs.mcp_file());
    assert_eq!(mcp["other"], 1);
    assert_eq!(mcp["mcpServers"]["mine"], json!({ "command": "x" }));
    let db = &mcp["mcpServers"]["plugin_fmt_db"];
    assert_eq!(db["claudePlugin"], "fmt@acme");
    assert_eq!(db["args"], json!(["--data", data.display().to_string()]));

    let config = read_json(&dirs.config_file());
    assert_eq!(config["plugins"]["fmt@acme"], json!({ "enabled": true }));
    assert_eq!(config["marketplaces"]["acme"]["autoUpdate"], false);

    let text = install::uninstall(&env, "fmt@acme", None).unwrap();
    assert!(text.contains("Uninstalled fmt@acme [user]"), "{text}");
    assert!(names(&dirs.prompts()).is_empty());
    assert!(names(&dirs.skills()).is_empty());
    assert!(
        !fx.agent
            .join("ext/subagents/agents/fmt-reviewer.md")
            .exists()
    );
    let hooks = read_json(&dirs.hooks_file());
    assert_eq!(hooks["hooks"]["PreToolUse"], json!([user_group]));
    let mcp = read_json(&dirs.mcp_file());
    assert_eq!(mcp["mcpServers"], json!({ "mine": { "command": "x" } }));
    assert!(State::load(&dirs).unwrap().plugins.is_empty());
    assert!(
        read_json(&dirs.config_file())["plugins"]
            .get("fmt@acme")
            .is_none()
    );
}

#[test]
fn project_scope_needs_trust_and_substitutes_project_dir() {
    let fx = Fixture::new();
    market::add(&fx.env(true), fx.repo.to_str().unwrap(), Scope::User).unwrap();
    let err = install::install(&fx.env(false), "fmt@acme", Scope::Project).unwrap_err();
    assert!(format!("{err:#}").contains("trusted project"), "{err:#}");

    let env = fx.env(true);
    let text = install::install(&env, "lint@acme", Scope::Project).unwrap();
    assert!(text.contains("Not supported in pi: lspServers"), "{text}");
    let dirs = Dirs::new(&fx.project.join(".pi"));
    assert_eq!(names(&dirs.prompts()), ["lint:run.md"]);
    assert!(read(&dirs.skills().join("lint-rules/SKILL.md")).contains("name: lint-rules"));
    let agent = read(&fx.project.join(".pi/ext/subagents/agents/lint-linter.md"));
    assert!(agent.contains("tools: grep, ls\n"), "{agent}");
    let hooks = read_json(&fx.project.join(".pi/ext/hooks.json"));
    let data = dirs.plugin_data("acme", "lint").display().to_string();
    assert_eq!(
        hooks["hooks"]["Stop"][0]["hooks"][0]["command"],
        format!("echo {data}")
    );
    let mcp = read_json(&fx.project.join(".pi/mcp.json"));
    assert_eq!(
        mcp["mcpServers"]["plugin_lint_srv"]["url"],
        "http://127.0.0.1:1/mcp"
    );

    install::install(&env, "fmt@acme", Scope::Project).unwrap();
    let agent = read(&fx.project.join(".pi/ext/subagents/agents/fmt-reviewer.md"));
    assert!(
        agent.contains(&format!("Review in {}", fx.project.display())),
        "{agent}"
    );
    assert!(!fx.agent.join("ext/claude-marketplace/resources").exists());
}

#[test]
fn update_after_new_commit_reinstalls() {
    let fx = Fixture::new();
    let env = fx.env(true);
    market::add(&env, &fx.file_url(), Scope::User).unwrap();
    let dirs = Dirs::new(&fx.agent);
    assert!(dirs.sources().join("acme/.git").is_dir());
    install::install(&env, "fmt@acme", Scope::User).unwrap();
    let clone_root = dirs.sources().join("acme/plugins/fmt");
    assert_eq!(
        read(&dirs.prompts().join("fmt:hello.md")),
        "---\ndescription: Say hello\n---\nHello $ARGUMENTS\n"
    );

    common::fixture::write(&fx.repo.join("plugins/fmt/commands/hello.md"), "Hi again\n");
    common::fixture::write(&fx.repo.join("plugins/fmt/commands/bye.md"), "Bye\n");
    fx.commit("more");
    let text = cli::run(&env, "update").unwrap();
    assert!(text.contains("Updated marketplace acme [user]"), "{text}");
    assert!(text.contains("Installed fmt@acme"), "{text}");
    assert_eq!(read(&dirs.prompts().join("fmt:hello.md")), "Hi again\n");
    assert_eq!(read(&dirs.prompts().join("fmt:bye.md")), "Bye\n");
    let hooks = read_json(&dirs.hooks_file());
    assert_eq!(hooks["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    assert_eq!(
        hooks["hooks"]["PreToolUse"][0]["pluginRoot"],
        clone_root.display().to_string()
    );

    let text = market::remove(&env, "acme", None).unwrap();
    assert!(text.contains("Uninstalled fmt@acme [user]"), "{text}");
    assert!(!dirs.sources().join("acme").exists());
    assert!(names(&dirs.prompts()).is_empty());
    assert_eq!(read_json(&dirs.hooks_file())["hooks"], json!({}));
}

#[test]
fn disable_and_enable() {
    let fx = Fixture::new();
    let env = fx.env(true);
    market::add(&env, fx.repo.to_str().unwrap(), Scope::User).unwrap();
    install::install(&env, "fmt@acme", Scope::User).unwrap();
    let dirs = Dirs::new(&fx.agent);
    let text = cli::run(&env, "disable fmt@acme").unwrap();
    assert!(text.starts_with("Disabled fmt@acme [user]"), "{text}");
    assert!(names(&dirs.prompts()).is_empty());
    assert!(
        read_json(&dirs.mcp_file())["mcpServers"]
            .get("plugin_fmt_db")
            .is_none()
    );
    assert!(!State::load(&dirs).unwrap().plugins["fmt@acme"].enabled);
    assert_eq!(
        read_json(&dirs.config_file())["plugins"]["fmt@acme"]["enabled"],
        false
    );
    let list = cli::run(&env, "list acme").unwrap();
    assert!(
        list.contains("[unavailable] fmt  1.0.0  (disabled)"),
        "{list}"
    );

    let text = cli::run(&env, "enable fmt@acme").unwrap();
    assert!(text.starts_with("Enabled fmt@acme"), "{text}");
    assert_eq!(names(&dirs.prompts()).len(), 2);
}

#[test]
fn failed_update_keeps_the_installed_plugin() {
    let fx = Fixture::new();
    let env = fx.env(true);
    market::add(&env, fx.repo.to_str().unwrap(), Scope::User).unwrap();
    install::install(&env, "fmt@acme", Scope::User).unwrap();
    let manifest = fx.repo.join("plugins/fmt/.claude-plugin/plugin.json");
    write_json(
        &manifest,
        &json!({ "name": "fmt", "commands": ["../../../etc"] }),
    );
    let text = cli::run(&env, "update").unwrap();
    assert!(text.contains("Cannot update fmt@acme"), "{text}");
    let dirs = Dirs::new(&fx.agent);
    assert_eq!(names(&dirs.prompts()), ["fmt:build:web.md", "fmt:hello.md"]);
    assert!(State::load(&dirs).unwrap().plugins.contains_key("fmt@acme"));
}
