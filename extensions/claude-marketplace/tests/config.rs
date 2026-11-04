mod common;

use std::path::Path;

use common::fixture::{Fixture, read, read_json, write, write_json};
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::state::State;
use pi_ext_claude_marketplace::{Env, Scope, cli, install, market, settings};
use serde_json::{Value, json};

fn plugin_app(root: &Path) {
    write_json(
        &root.join(".claude-plugin/plugin.json"),
        &json!({ "name": "app", "commands": "./cmds",
            "userConfig": {
                "region": { "type": "string", "title": "Region", "description": "R", "default": "eu" },
                "token": { "type": "string", "title": "Token", "description": "T", "sensitive": true, "default": "tok" }
            },
            "mcpServers": {
                "local": { "command": "${CLAUDE_PLUGIN_ROOT}/srv", "env": { "OWN": "1" } },
                "remote": { "type": "http", "url": "https://x/${CLAUDE_PLUGIN_OPTION_REGION}/mcp",
                    "headers": { "X-Team": "${TEAM}" } }
            } }),
    );
    write(&root.join("cmds/legacy.md"), "Old $1 and $2\n");
    write(&root.join("cmds/modern.md"), "New $0 and $1\n");
    write(
        &root.join("cmds/env.md"),
        "Root ${CLAUDE_PLUGIN_ROOT} team ${TEAM} region ${CLAUDE_PLUGIN_OPTION_REGION} token ${CLAUDE_PLUGIN_OPTION_TOKEN}\nNow !`date`\n",
    );
    write(&root.join("commands/other.md"), "Other $ARGUMENTS\n");
    write(
        &root.join("more-skills/extra/SKILL.md"),
        "---\nname: extra\n---\nX\n",
    );
    write(&root.join("tools/app-cli"), "#!/bin/sh\n");
    write(
        &root.join("agents/review.md"),
        "---\nname: review\nmodel: opus\neffort: low\n---\nR\n",
    );
    write(
        &root.join("agents/fast.md"),
        "---\nname: fast\nmodel: haiku\n---\nF\n",
    );
}

struct Market {
    fx: Fixture,
    user: Dirs,
    project: Dirs,
}

fn setup() -> Market {
    let fx = Fixture::new();
    let dir = fx.repo.parent().unwrap().join("tm");
    let catalog = json!({ "name": "tm", "owner": { "name": "T" }, "plugins": [
        { "name": "app", "source": "./app" },
        { "name": "clash", "source": "./clash", "strict": false, "commands": "./extra" }
    ]});
    write_json(&dir.join(".claude-plugin/marketplace.json"), &catalog);
    plugin_app(&dir.join("app"));
    write_json(
        &dir.join("clash/.claude-plugin/plugin.json"),
        &json!({ "name": "clash" }),
    );
    write(&dir.join("clash/extra/x.md"), "X $ARGUMENTS\n");
    market::add(&fx.env(true), dir.to_str().unwrap(), Scope::User).unwrap();
    let user = Dirs::new(&fx.agent);
    let project = Dirs::new(&fx.project.join(".pi"));
    Market { fx, user, project }
}

/// Edits the config file of `dirs` in place.
fn config(dirs: &Dirs, edit: impl FnOnce(&mut Value)) {
    let file = dirs.config_file();
    let mut value = if file.is_file() {
        read_json(&file)
    } else {
        json!({})
    };
    edit(&mut value);
    write_json(&file, &value);
}

fn strict_of(env: &Env, scope: Scope) -> Option<(bool, String)> {
    settings::load(env, scope, "clash@tm").unwrap().strict
}

#[test]
fn strict_override_precedence() {
    let m = setup();
    let env = m.fx.env(true);
    let err = install::install(&env, "clash@tm", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("strict is false (marketplace entry)"),
        "{err:#}"
    );

    config(&m.user, |c| c["strict"] = json!(false));
    assert_eq!(
        strict_of(&env, Scope::User),
        Some((false, "user default config".into()))
    );
    config(&m.user, |c| c["marketplaces"]["tm"]["strict"] = json!(true));
    assert_eq!(
        strict_of(&env, Scope::User),
        Some((true, "user marketplace config".into()))
    );
    config(&m.user, |c| {
        c["plugins"]["clash@tm"] = json!({ "strict": false })
    });
    assert_eq!(
        strict_of(&env, Scope::User),
        Some((false, "user plugin config".into()))
    );
    config(&m.project, |c| c["strict"] = json!(true));
    assert_eq!(
        strict_of(&env, Scope::User),
        Some((false, "user plugin config".into()))
    );
    assert_eq!(
        strict_of(&env, Scope::Project),
        Some((true, "project default config".into()))
    );
    config(&m.project, |c| {
        c["marketplaces"] = json!({ "tm": { "strict": false } })
    });
    let project = strict_of(&env, Scope::Project);
    assert_eq!(project, Some((false, "project marketplace config".into())));
    config(&m.project, |c| {
        c["plugins"] = json!({ "clash@tm": { "strict": true } })
    });
    let project = strict_of(&env, Scope::Project);
    assert_eq!(project, Some((true, "project plugin config".into())));

    let text = install::install(&env, "clash@tm", Scope::Project).unwrap();
    assert!(
        text.contains("1 command(s)"),
        "entry commands added: {text}"
    );
    let info = cli::info(&env, "clash@tm").unwrap();
    assert!(
        info.contains("Strict: true (project plugin config)"),
        "{info}"
    );
    let err = install::install(&env, "clash@tm", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("strict is false (user plugin config)"),
        "{err:#}"
    );
}

fn prompt_names(dirs: &Dirs) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dirs.prompts())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn config_paths_replace_and_add() {
    let m = setup();
    let env = m.fx.env(true);
    install::install(&env, "app@tm", Scope::User).unwrap();
    assert_eq!(
        prompt_names(&m.user),
        ["app:env.md", "app:legacy.md", "app:modern.md"]
    );
    config(&m.user, |c| {
        c["plugins"]["app@tm"] = json!({ "enabled": true,
            "paths": { "commands": ["./commands"] },
            "extraPaths": { "skills": ["./more-skills"], "bin": "./tools" } });
    });
    let text = cli::run(&env, "reinstall app@tm").unwrap();
    assert!(text.contains("1 command(s), 1 skill(s)"), "{text}");
    assert!(text.contains("1 bin command(s)"), "{text}");
    assert_eq!(prompt_names(&m.user), ["app:other.md"]);
    assert!(m.user.skills().join("app-extra/SKILL.md").is_file());
    let record = &State::load(&m.user).unwrap().plugins["app@tm"];
    assert_eq!(record.bins[0].names, ["app-cli"]);

    config(&m.user, |c| {
        c["plugins"]["app@tm"]["paths"]["agents"] = json!("../../etc")
    });
    let err = install::install(&env, "app@tm", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("leaves the plugin root"),
        "{err:#}"
    );
    config(&m.user, |c| {
        c["plugins"]["app@tm"]["paths"] = json!({ "lsp": "./x" })
    });
    let err = install::install(&env, "app@tm", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("paths.lsp is not a component kind"),
        "{err:#}"
    );

    install::uninstall(&env, "app@tm", None).unwrap();
    let kept = &read_json(&m.user.config_file())["plugins"]["app@tm"];
    assert_eq!(
        kept["enabled"], false,
        "overrides survive uninstall: {kept}"
    );
    assert!(kept.get("extraPaths").is_some());
}

#[test]
fn env_reaches_mcp_servers_commands_and_bash() {
    let m = setup();
    let env = m.fx.env(true);
    config(&m.user, |c| {
        c["env"] = json!({ "TEAM": "core", "LOG": "1" });
        c["plugins"]["app@tm"] = json!({ "env": { "TEAM": "app-team" },
            "extraPaths": { "bin": ["./tools"] } });
    });
    let text = install::install(&env, "app@tm", Scope::User).unwrap();
    assert!(
        text.contains(
            "!`cmd` shell injection (app:env) (pi prompt templates do not run shell commands"
        ),
        "{text}"
    );
    let mcp = read_json(&m.user.mcp_file());
    let local = &mcp["mcpServers"]["plugin_app_local"]["env"];
    let root = m.fx.repo.parent().unwrap().join("tm/app");
    assert_eq!(local["OWN"], "1");
    assert_eq!(local["CLAUDE_PLUGIN_ROOT"], root.display().to_string());
    assert!(local["CLAUDE_PLUGIN_DATA"].is_string(), "{local}");
    assert_eq!(local["CLAUDE_PLUGIN_OPTION_REGION"], "eu");
    assert_eq!(local["CLAUDE_PLUGIN_OPTION_TOKEN"], "tok");
    assert_eq!(local["TEAM"], "app-team");
    assert_eq!(local["LOG"], "1");
    let remote = &mcp["mcpServers"]["plugin_app_remote"];
    assert_eq!(remote["url"], "https://x/eu/mcp");
    assert_eq!(remote["headers"]["X-Team"], "app-team");
    assert!(remote.get("env").is_none(), "{remote}");

    let prompt = read(&m.user.prompts().join("app:env.md"));
    let expected = format!(
        "Root {} team app-team region eu token ${{CLAUDE_PLUGIN_OPTION_TOKEN}}",
        root.display()
    );
    assert!(prompt.starts_with(&expected), "{prompt}");

    let record = &State::load(&m.user).unwrap().plugins["app@tm"];
    let script = read(record.env_file.as_ref().unwrap());
    assert!(
        script.contains("export CLAUDE_PLUGIN_OPTION_TOKEN='tok'\n"),
        "{script}"
    );
    assert!(script.contains("export TEAM='app-team'\n"), "{script}");

    config(&m.project, |c| c["env"] = json!({ "TEAM": "proj" }));
    install::install(&env, "app@tm", Scope::Project).unwrap();
    let mcp = read_json(&m.project.mcp_file());
    let local = &mcp["mcpServers"]["plugin_app_local"]["env"];
    assert_eq!(local["TEAM"], "proj", "project default beats user plugin");
    assert_eq!(
        local["CLAUDE_PROJECT_DIR"],
        m.fx.project.display().to_string()
    );
}

fn agent(dirs: &Dirs, name: &str) -> String {
    read(&dirs.agents().join(format!("{name}.md")))
}

#[test]
fn agent_model_rules() {
    let m = setup();
    let env = m.fx.env(true);
    install::install(&env, "app@tm", Scope::User).unwrap();
    let review = agent(&m.user, "app-review");
    assert!(
        review.contains("model: inherit\n") && review.contains("thinking: low\n"),
        "{review}"
    );

    config(&m.user, |c| {
        c["agentModels"] = json!([
            { "match": { "model": "opus*" }, "model": "openai/gpt-5", "thinking": "medium" },
            { "match": { "agent": "*" }, "model": "never" }
        ]);
        c["plugins"]["app@tm"] = json!({ "agentModels": [
            { "match": { "agent": "fast" }, "provider": "ollama", "model": "qwen3" }
        ] });
    });
    install::install(&env, "app@tm", Scope::User).unwrap();
    let review = agent(&m.user, "app-review");
    assert!(review.contains("model: openai/gpt-5\n"), "{review}");
    assert!(review.contains("thinking: medium\n"), "{review}");
    assert!(agent(&m.user, "app-fast").contains("model: ollama/qwen3\n"));
    let info = cli::info(&env, "app@tm").unwrap();
    for line in [
        "  app-fast: model haiku, effort none -> model ollama/qwen3, thinking inherit (rule 1 of the user plugin config)",
        "  app-review: model opus, effort low -> model openai/gpt-5, thinking medium (rule 1 of the user default config)",
    ] {
        assert!(info.contains(line), "missing {line:?} in {info}");
    }

    config(&m.project, |c| {
        c["agentModels"] =
            json!([{ "match": { "plugin": "app", "agent": "app-rev*" }, "model": "inherit" }]);
    });
    install::install(&env, "app@tm", Scope::Project).unwrap();
    let review = agent(&m.project, "app-review");
    assert!(
        review.contains("model: inherit\n"),
        "project rule first: {review}"
    );
    assert!(
        review.contains("thinking: low\n"),
        "built-in thinking stays: {review}"
    );
}

#[test]
fn argument_indexing_switch() {
    let m = setup();
    let env = m.fx.env(true);
    install::install(&env, "app@tm", Scope::User).unwrap();
    let prompt = |name: &str| read(&m.user.prompts().join(format!("app:{name}.md")));
    assert_eq!(
        prompt("legacy"),
        "Old $1 and $2\n",
        "auto: no $0, so 1-based"
    );
    assert_eq!(prompt("modern"), "New $1 and $2\n", "auto: $0, so 0-based");
    let info = cli::info(&env, "app@tm").unwrap();
    assert!(info.contains("Argument indexing: auto (default)"), "{info}");

    config(&m.user, |c| {
        c["plugins"]["app@tm"] = json!({ "argumentIndex": "zero" })
    });
    install::install(&env, "app@tm", Scope::User).unwrap();
    assert_eq!(prompt("legacy"), "Old $2 and $3\n");
    config(&m.user, |c| {
        c["plugins"]["app@tm"]["argumentIndex"] = json!("one");
        c["argumentIndex"] = json!("zero");
    });
    install::install(&env, "app@tm", Scope::User).unwrap();
    assert_eq!(
        prompt("modern"),
        "New $0 and $1\n",
        "the plugin key beats the default"
    );
    let info = cli::info(&env, "app@tm").unwrap();
    assert!(
        info.contains("Argument indexing: one (user plugin config)"),
        "{info}"
    );

    config(&m.user, |c| {
        c["plugins"]["app@tm"]["argumentIndex"] = json!("two")
    });
    let err = install::install(&env, "app@tm", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("use auto, zero, or one"),
        "{err:#}"
    );
}
