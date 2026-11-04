mod common;

use std::path::{Path, PathBuf};

use common::fixture::{Fixture, git, read, write, write_json};
use pi_ext_claude_marketplace::env::Dirs;
use pi_ext_claude_marketplace::state::State;
use pi_ext_claude_marketplace::{Scope, cli, install, market};
use serde_json::json;

fn base(fx: &Fixture) -> PathBuf {
    fx.repo.parent().unwrap().to_path_buf()
}

fn skill(dir: &Path, name: &str) {
    write(
        &dir.join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: The {name} skill\n---\nUse {name}.\n"),
    );
    write(&dir.join("ref.md"), "ref\n");
}

fn skill_names(dirs: &Dirs) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dirs.skills())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn single_skill_folder() {
    let fx = Fixture::new();
    let env = fx.env(true);
    let dir = base(&fx).join("PDF Tools");
    skill(&dir, "pdf");
    let text = market::add(&env, dir.to_str().unwrap(), Scope::User).unwrap();
    assert!(
        text.starts_with("Added marketplace pdf-tools (1 plugin(s), from a single skill) in user scope. Install with /claude-plugins install pdf@pdf-tools."),
        "{text}"
    );
    let text = install::install(&env, "pdf@pdf-tools", Scope::User).unwrap();
    assert!(
        text.contains("Installed pdf@pdf-tools [user]: 0 command(s), 1 skill(s)"),
        "{text}"
    );
    let dirs = Dirs::new(&fx.agent);
    assert_eq!(skill_names(&dirs), ["pdf"]);
    assert!(dirs.skills().join("pdf/ref.md").is_file());
    let list = cli::run(&env, "list pdf-tools").unwrap();
    assert!(list.contains("[installed] pdf"), "{list}");

    write(&dir.join("SKILL.md"), "---\nname: pdf\n---\nNew text.\n");
    cli::run(&env, "update pdf@pdf-tools").unwrap();
    assert!(read(&dirs.skills().join("pdf/SKILL.md")).contains("New text."));
    install::uninstall(&env, "pdf@pdf-tools", None).unwrap();
    assert!(skill_names(&dirs).is_empty());
    let text = market::remove(&env, "pdf-tools", None).unwrap();
    assert!(text.contains("Removed marketplace pdf-tools"), "{text}");
    assert!(dir.join("SKILL.md").is_file(), "a local source stays");
}

#[test]
fn folders_of_skills() {
    let fx = Fixture::new();
    let env = fx.env(true);
    let flat = base(&fx).join("flat");
    skill(&flat.join("alpha"), "alpha");
    skill(&flat.join("beta"), "beta");
    write(&flat.join("notes/README.md"), "not a skill\n");
    let nested = base(&fx).join("nested");
    skill(&nested.join("skills/gamma"), "gamma");

    let text = install::install_source(&env, flat.to_str().unwrap(), Scope::User).unwrap();
    assert!(text.contains("from a folder of skills"), "{text}");
    assert!(
        text.contains("Installed flat@flat [user]: 0 command(s), 2 skill(s)"),
        "{text}"
    );
    let text = cli::run(&env, &format!("install {}", nested.display())).unwrap();
    assert!(
        text.contains("Installed nested@nested [user]: 0 command(s), 1 skill(s)"),
        "{text}"
    );
    let dirs = Dirs::new(&fx.agent);
    assert_eq!(
        skill_names(&dirs),
        ["flat-alpha", "flat-beta", "nested-gamma"]
    );

    let again = install::install_source(&env, flat.to_str().unwrap(), Scope::User).unwrap();
    assert!(
        !again.contains("Added marketplace"),
        "reuses the marketplace: {again}"
    );
    assert!(again.starts_with("Installed flat@flat"), "{again}");

    let empty = base(&fx).join("empty");
    write(&empty.join("README.md"), "x\n");
    let err = market::add(&env, empty.to_str().unwrap(), Scope::User).unwrap_err();
    assert!(
        format!("{err:#}")
            .contains("has no .claude-plugin/marketplace.json, plugin.json, or SKILL.md"),
        "{err:#}"
    );
}

#[test]
fn plugin_folder_and_git_skill_repo() {
    let fx = Fixture::new();
    let env = fx.env(true);
    let plugin = base(&fx).join("tooling");
    write_json(
        &plugin.join(".claude-plugin/plugin.json"),
        &json!({ "name": "tool", "description": "Tools" }),
    );
    write(&plugin.join("commands/run.md"), "Run $ARGUMENTS\n");
    let text = install::install_source(&env, plugin.to_str().unwrap(), Scope::User).unwrap();
    assert!(
        text.contains("(1 plugin(s), from a plugin folder)"),
        "{text}"
    );
    assert!(
        text.contains("Installed tool@tooling [user]: 1 command(s)"),
        "{text}"
    );

    let repo = base(&fx).join("src/notes-skill");
    skill(&repo, "notes");
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let url = format!("file://{}", repo.display());
    let text = cli::run(&env, &format!("install {url}")).unwrap();
    assert!(
        text.contains("Added marketplace notes-skill (1 plugin(s), from a single skill)"),
        "{text}"
    );
    assert!(
        text.contains("Installed notes@notes-skill [user]"),
        "{text}"
    );
    let dirs = Dirs::new(&fx.agent);
    assert!(dirs.sources().join("notes-skill/SKILL.md").is_file());

    write(
        &repo.join("SKILL.md"),
        "---\nname: notes\n---\nVersion two.\n",
    );
    git(&repo, &["commit", "-q", "-am", "two"]);
    let text = cli::run(&env, "marketplace update notes-skill").unwrap();
    assert!(text.contains("Installed notes@notes-skill"), "{text}");
    assert!(read(&dirs.skills().join("notes/SKILL.md")).contains("Version two."));
    market::remove(&env, "notes-skill", None).unwrap();
    assert!(!dirs.sources().join("notes-skill").exists());
    assert!(
        !State::load(&dirs)
            .unwrap()
            .plugins
            .contains_key("notes@notes-skill")
    );
}
