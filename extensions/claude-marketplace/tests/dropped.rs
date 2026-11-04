mod common;

use common::fixture::{Fixture, write, write_json};
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::state::State;
use pi_ext_claude_marketplace::{Scope, cli, install, market};
use serde_json::{Value, json};

/// Fields the extension applies, from the Claude Code plugin reference.
const APPLIED: [&str; 15] = [
    "name",
    "version",
    "description",
    "commands",
    "agents",
    "skills",
    "hooks",
    "mcpServers",
    "lspServers",
    "outputStyles",
    "userConfig",
    "dependencies",
    "source",
    "strict",
    "experimental",
];

/// Every plugin.json field of the Claude Code plugin reference.
fn manifest() -> Value {
    json!({
        "$schema": "https://example.com/plugin.schema.json",
        "name": "all", "displayName": "All", "version": "1.0.0", "description": "Everything",
        "author": { "name": "A" }, "homepage": "https://example.com", "repository": "x/y",
        "license": "MIT", "keywords": ["k"], "metadata": { "k": 1 }, "icon": "./icon.png",
        "documentationUrl": "https://d", "supportUrl": "https://s",
        "privacyPolicyUrl": "https://p", "termsOfServiceUrl": "https://t",
        "defaultEnabled": false,
        "dependencies": [{ "name": "dep", "version": "^1.0.0" }],
        "settings": { "agent": "helper" },
        "userConfig": { "k": { "type": "string", "title": "K", "description": "K", "default": "v" } },
        "channels": [{ "server": "srv" }],
        "types": "./types.d.ts",
        "commands": "./commands", "agents": "./agents", "skills": "./skills",
        "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "true" }] }] },
        "mcpServers": ["./servers.json", "./bundle.mcpb"],
        "lspServers": { "go": { "command": "gopls", "extensionToLanguage": { ".go": "go" } } },
        "outputStyles": "./output-styles", "workflows": "./workflows", "themes": "./themes",
        "monitors": [{ "name": "m", "command": "tail", "description": "d" }],
        "experimental": { "themes": "./themes", "monitors": [], "evals": "./evals" }
    })
}

/// Every marketplace entry field that is not also a plugin.json field.
fn entry() -> Value {
    json!({ "name": "all", "source": "./all", "description": "Entry", "version": "1.0.0",
        "category": "dev", "tags": ["t"], "strict": true, "relevance": 1,
        "dependencies": ["dep"], "defaultEnabled": true, "displayName": "All",
        "metadata": {}, "headers": {}, "headersHelper": "./h.sh",
        "mcpServers": { "x": { "command": "x" } }, "lspServers": {}, "userConfig": {},
        "channels": [] })
}

fn fixture(fx: &Fixture) -> std::path::PathBuf {
    let dir = fx.repo.parent().unwrap().join("everything");
    let catalog = json!({ "$schema": "s", "name": "every", "owner": { "name": "O" },
        "description": "D", "version": "1", "metadata": { "description": "d", "pluginRoot": "." },
        "forceRemoveDeletedPlugins": true, "allowCrossMarketplaceDependenciesOn": ["x"],
        "renames": { "old": "all" },
        "plugins": [entry(), { "name": "dep", "source": "./dep" }] });
    write_json(&dir.join(".claude-plugin/marketplace.json"), &catalog);
    let root = dir.join("all");
    write_json(&root.join(".claude-plugin/plugin.json"), &manifest());
    write(
        &root.join("commands/run.md"),
        "---\ndescription: Run\nallowed-tools: Bash(git:*)\nmodel: opus\ndisable-model-invocation: true\n---\nSee @docs/a.md and $ARGUMENTS\n",
    );
    write(
        &root.join("skills/s/SKILL.md"),
        "---\nname: s\ndescription: S\nmodel: opus\neffort: high\ncontext: fork\nhooks: {}\n---\nS\n",
    );
    write(
        &root.join("agents/helper.md"),
        "---\nname: helper\ncolor: red\nmemory: user\nskills: s\npermissionMode: plan\nisolation: worktree\nbackground: true\n---\nH\n",
    );
    write(
        &root.join("output-styles/brief.md"),
        "---\nname: brief\nkeep-coding-instructions: true\n---\nBrief.\n",
    );
    write_json(
        &root.join("servers.json"),
        &json!({ "mcpServers": { "srv": { "command": "s" } } }),
    );
    write(&root.join("bundle.mcpb"), "zip");
    write(&root.join(".lsp.json"), "{}");
    write(&root.join("monitors/monitors.json"), "[]");
    write(&root.join("themes/dark.json"), "{}");
    write(&root.join("workflows/w.js"), "");
    write(&root.join("evals/e.json"), "{}");
    write_json(&root.join("settings.json"), &json!({ "agent": "helper" }));
    write_json(
        &dir.join("dep/.claude-plugin/plugin.json"),
        &json!({ "name": "dep" }),
    );
    dir
}

#[test]
fn every_known_field_is_applied_or_reported() {
    let fx = Fixture::new();
    let env = fx.env(true);
    let dir = fixture(&fx);
    let added = market::add(&env, dir.to_str().unwrap(), Scope::User).unwrap();
    for field in [
        "$schema",
        "owner",
        "description",
        "version",
        "metadata.description",
        "forceRemoveDeletedPlugins",
        "allowCrossMarketplaceDependenciesOn",
        "renames",
    ] {
        assert!(
            added.contains(field),
            "marketplace field {field} not reported: {added}"
        );
    }
    assert!(
        !added.contains("pluginRoot"),
        "pluginRoot is applied: {added}"
    );

    let text = install::install(&env, "all@every", Scope::User).unwrap();
    let dirs = Dirs::new(&fx.agent);
    let record = State::load(&dirs).unwrap().plugins["all@every"].clone();
    let whats: Vec<&str> = record.dropped.iter().map(|d| d.what.as_str()).collect();
    assert_eq!(record.lsp_keys, ["plugin_all_go"]);
    for d in &record.dropped {
        assert!(!d.reason.is_empty(), "{d:?}");
        assert_ne!(
            d.reason, "unknown field",
            "a known field is not classified: {d:?}"
        );
    }
    let mut expected: Vec<String> = Vec::new();
    for (origin, value) in [("plugin.json", manifest()), ("marketplace entry", entry())] {
        for (key, inner) in value.as_object().unwrap() {
            if key == "experimental" {
                let nested = inner.as_object().unwrap().keys();
                expected.extend(nested.map(|k| format!("{origin} experimental.{k}")));
            } else if !APPLIED.contains(&key.as_str()) {
                expected.push(format!("{origin} {key}"));
            }
        }
    }
    for key in ["mcpServers", "lspServers", "userConfig", "channels"] {
        expected.push(format!("marketplace entry {key}"));
    }
    expected.extend(
        [
            "mcpServers \"./bundle.mcpb\"",
            "dependency dep version ^1.0.0",
            "monitors/monitors.json",
            "themes/",
            "workflows/",
            "evals/",
            "settings.json",
            "command frontmatter allowed-tools (all:run)",
            "command frontmatter model (all:run)",
            "command frontmatter disable-model-invocation (all:run)",
            "@file references (all:run)",
            "skill frontmatter model (all-s)",
            "skill frontmatter effort (all-s)",
            "skill frontmatter context (all-s)",
            "skill frontmatter hooks (all-s)",
            "agent frontmatter color (all-helper)",
            "agent frontmatter memory (all-helper)",
            "agent frontmatter skills (all-helper)",
            "agent frontmatter permissionMode (all-helper)",
            "agent frontmatter isolation (all-helper)",
            "agent frontmatter background (all-helper)",
            "style frontmatter keep-coding-instructions (all:brief)",
        ]
        .map(str::to_owned),
    );
    let info = cli::info(&env, "all@every").unwrap();
    for item in &expected {
        assert!(
            whats.contains(&item.as_str()),
            "{item} is dropped silently; got {whats:?}"
        );
        assert!(
            text.contains(item.as_str()),
            "{item} missing from install text: {text}"
        );
        assert!(
            info.contains(item.as_str()),
            "{item} missing from info: {info}"
        );
    }
    assert!(
        text.contains("pi skills have no model or thinking setting"),
        "{text}"
    );
    assert!(text.contains("Dependency: Installed dep@every"), "{text}");
}
