mod common;

use common::fixture::{Fixture, read_json, write, write_json};
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::state::State;
use pi_ext_claude_marketplace::{Scope, cli, complete, import, install, market};
use serde_json::json;

#[test]
fn import_from_claude_settings() {
    let fx = Fixture::new();
    let env = fx.env(true);
    let known = json!({ "acme": { "source": { "source": "directory", "path": fx.repo } } });
    write_json(
        &fx.claude.join("settings.json"),
        &json!({
            "enabledPlugins": { "fmt@acme": true, "pkg@acme": false },
            "extraKnownMarketplaces": known
        }),
    );
    write_json(
        &fx.claude.join("settings.local.json"),
        &json!({ "enabledPlugins": { "lint@acme": true } }),
    );
    let text = import::import(&env, Some(Scope::User)).unwrap();
    assert!(text.contains("Added marketplace acme"), "{text}");
    assert!(text.contains("Installed fmt@acme"), "{text}");
    assert!(text.contains("Installed lint@acme"), "{text}");
    assert!(
        text.ends_with("/reload to apply the changes in pi."),
        "{text}"
    );
    let state = State::load(&Dirs::new(&fx.agent)).unwrap();
    let ids: Vec<&String> = state.plugins.keys().collect();
    assert_eq!(ids, ["fmt@acme", "lint@acme"]);
    let config = read_json(&Dirs::new(&fx.agent).config_file());
    assert_eq!(
        config["plugins"],
        json!({ "fmt@acme": { "enabled": true }, "lint@acme": { "enabled": true } })
    );

    let again = import::import(&env, Some(Scope::User)).unwrap();
    assert_eq!(again, "Nothing to import from the Claude Code settings.");
}

#[test]
fn rejects_paths_that_leave_the_plugin() {
    let fx = Fixture::new();
    let env = fx.env(true);
    let fmt = fx.repo.join("plugins/fmt/.claude-plugin/plugin.json");
    write_json(
        &fmt,
        &json!({ "name": "fmt", "commands": ["../../../etc"] }),
    );
    write(&fx.repo.join("plugins/lint/plugin.json"), "{ not json");
    market::add(&env, fx.repo.to_str().unwrap(), Scope::User).unwrap();

    let err = install::install(&env, "fmt@acme", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("leaves the plugin root"),
        "{err:#}"
    );
    let err = install::install(&env, "escape@acme", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("leaves the plugin root"),
        "{err:#}"
    );
    let err = install::install(&env, "pkg@acme", Scope::User).unwrap_err();
    assert!(
        format!("{err:#}").contains("npm plugin sources are not supported"),
        "{err:#}"
    );
    write_json(&fmt, &json!({ "name": "fmt", "skills": "/etc" }));
    let err = install::install(&env, "fmt@acme", Scope::User).unwrap_err();
    assert!(format!("{err:#}").contains("must be relative"), "{err:#}");

    let list = cli::run(&env, "list --unavailable").unwrap();
    assert!(list.starts_with("Marketplace acme (user)\n"), "{list}");
    assert!(list.contains("[unavailable] lint  0.1.0  ("), "{list}");
    assert!(list.contains("is not valid JSON"), "{list}");
    assert!(
        list.contains("[unavailable] pkg  (npm plugin sources are not supported)"),
        "{list}"
    );
    assert!(list.contains("[unavailable] escape"), "{list}");
    assert!(
        State::load(&Dirs::new(&fx.agent))
            .unwrap()
            .plugins
            .is_empty()
    );
    assert!(!Dirs::new(&fx.agent).hooks_file().exists());
}

#[test]
fn marketplace_list_and_completions() {
    let fx = Fixture::new();
    let env = fx.env(true);
    assert_eq!(
        cli::run(&env, "marketplace list").unwrap(),
        "No marketplaces configured."
    );
    market::add(&env, fx.repo.to_str().unwrap(), Scope::User).unwrap();
    let err = market::add(&env, fx.repo.to_str().unwrap(), Scope::User).unwrap_err();
    assert!(err.to_string().contains("already added"), "{err}");
    let manifest = fx.repo.join(".claude-plugin/marketplace.json");
    let line = format!("[user] acme -- 4 plugin(s) -- {}", manifest.display());
    assert_eq!(cli::run(&env, "marketplace list").unwrap(), line);
    install::install(&env, "fmt@acme", Scope::User).unwrap();

    assert_eq!(complete::complete(&env, "inst"), ["install"]);
    assert_eq!(complete::complete(&env, "install "), ["install lint@acme"]);
    assert_eq!(
        complete::complete(&env, "uninstall "),
        ["uninstall fmt@acme"]
    );
    assert_eq!(
        complete::complete(&env, "marketplace re"),
        ["marketplace remove"]
    );
    assert_eq!(
        complete::complete(&env, "marketplace update "),
        ["marketplace update acme"]
    );
    assert_eq!(
        complete::complete(&env, "install fmt@acme --scope p"),
        ["install fmt@acme --scope project"]
    );
    let help = cli::run(&env, "help").unwrap();
    assert!(help.contains("/reload"));
    assert!(cli::run(&env, "frobnicate").is_err());
}
