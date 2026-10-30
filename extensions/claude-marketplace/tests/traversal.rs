mod common;

use std::path::PathBuf;

use common::fixture::{Fixture, read, write, write_json};
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::{Scope, catalog::Catalog, git, install, market};
use serde_json::json;

const TRAVERSAL: &str = "../../../../../victim";

fn victim(fx: &Fixture) -> PathBuf {
    let victim = fx.agent.parent().unwrap().join("victim");
    write(&victim.join("precious.txt"), "keep me");
    victim
}

fn add_market(fx: &Fixture, plugins: serde_json::Value) {
    write_json(
        &fx.repo.join(".claude-plugin/marketplace.json"),
        &json!({ "name": "acme", "plugins": plugins }),
    );
    market::add(&fx.env(true), fx.repo.to_str().unwrap(), Scope::User).unwrap();
}

#[test]
fn catalog_skips_traversal_plugin_names() {
    let value = json!({ "name": "acme", "plugins": [
        { "name": TRAVERSAL, "source": "./p" },
        { "name": "a/b", "source": "./p" },
        { "name": "ok", "source": "./p" }
    ]});
    let catalog = Catalog::from_value(&value, std::path::Path::new("/m")).unwrap();
    let names: Vec<&str> = catalog.plugins.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["ok"]);
}

#[test]
fn install_rejects_traversal_names() {
    let fx = Fixture::new();
    let victim = victim(&fx);
    add_market(
        &fx,
        json!([
            { "name": TRAVERSAL, "source": "./plugins/fmt" },
            { "name": TRAVERSAL, "source": { "source": "url", "url": fx.file_url() } }
        ]),
    );
    let err = install::install(&fx.env(true), &format!("{TRAVERSAL}@acme"), Scope::User)
        .unwrap_err()
        .to_string();
    assert!(err.contains("invalid plugin name"), "{err}");
    assert_eq!(read(&victim.join("precious.txt")), "keep me");
    assert!(!Dirs::new(&fx.agent).state_dir.join("data").exists());
}

#[test]
fn uninstall_does_not_delete_outside_the_state_dir() {
    let fx = Fixture::new();
    let victim = victim(&fx);
    let outside = fx.agent.parent().unwrap().join("outside.md");
    write(&outside, "keep me");
    let dirs = Dirs::new(&fx.agent);
    write_json(
        &dirs.state_file(),
        &json!({ "plugins": { "x@acme": {
            "plugin": TRAVERSAL, "marketplace": "acme", "marketplaceScope": "user",
            "enabled": true, "pluginRoot": "/nowhere", "files": [outside]
        } } }),
    );
    let err = install::uninstall(&fx.env(true), "x@acme", None).unwrap_err();
    assert!(err.to_string().contains("invalid plugin name"), "{err}");
    assert_eq!(read(&victim.join("precious.txt")), "keep me");
    assert_eq!(read(&outside), "keep me");
}

#[test]
fn rejects_shas_that_are_not_hex() {
    for sha in ["--output=/tmp/x", "-c", "abc", "zzzzzzz", ""] {
        assert!(git::check_sha(sha).is_err(), "{sha}");
    }
    git::check_sha("0123456789abcdef0123456789ABCDEF01234567").unwrap();

    let fx = Fixture::new();
    add_market(
        &fx,
        json!([{ "name": "fmt", "source":
            { "source": "url", "url": fx.file_url(), "sha": "--detach" } }]),
    );
    let err = install::install(&fx.env(true), "fmt@acme", Scope::User)
        .unwrap_err()
        .to_string();
    assert!(err.contains("invalid commit sha"), "{err}");
    assert!(
        !Dirs::new(&fx.agent)
            .state_dir
            .join("plugin-clones/acme/fmt")
            .exists()
    );
}
