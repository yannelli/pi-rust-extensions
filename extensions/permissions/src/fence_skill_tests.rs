//! pi skill folders under the read block: a project or user skill folder that is a symlink to the agent or home
//! dir opens nothing, a symlink inside a skill folder cannot reach out, and a real project skill outside the cwd reads.

use std::path::Path;

use serde_json::json;

use super::tests::{Fixture, fixture, write};
use super::*;
use crate::evaluate::{Call, evaluate};
use crate::fence::fence;
use crate::guards::Guard;

/// The block on, with the cwd in `pkg/` below the git root, so project skill folders lie outside it.
fn nested(fx: &Fixture) -> Loaded {
    let settings = json!({ "permissions": { "blockReadsOutsideWorkingDirectories": true } });
    write(
        &fx.paths.claude_dir.join("settings.json"),
        &settings.to_string(),
    );
    std::fs::create_dir_all(fx.cwd.join("pkg")).unwrap();
    let env = Env {
        paths: &fx.paths,
        agent_dir: fx.agent.clone(),
        cwd: fx.cwd.join("pkg"),
        trusted: true,
    };
    load(&env)
}

fn read(fx: &Fixture, loaded: &Loaded, path: &Path) -> Option<Guard> {
    let input = json!({ "path": path });
    let call = Call {
        tool: "read",
        input: &input,
        cwd: &loaded.cwd,
        home: &fx.paths.home,
        dirs: &loaded.dirs,
    };
    fence(loaded, &call, &evaluate(&loaded.rules, &call))
}

fn denied(guard: Option<Guard>) -> bool {
    matches!(guard, Some(Guard::Deny(_)))
}

#[test]
fn a_real_project_skill_reads_and_a_symlink_inside_it_cannot_reach_the_agent_dir() {
    let fx = fixture();
    let skill = fx.cwd.join(".agents/skills/z/SKILL.md");
    write(&skill, "skill");
    write(&fx.agent.join("auth.json"), "secret");
    let loaded = nested(&fx);
    assert_eq!(
        read(&fx, &loaded, &skill),
        None,
        "a project skill above the cwd, inside the git root"
    );
    #[cfg(unix)]
    {
        let link = fx.cwd.join(".agents/skills/z/auth.md");
        std::os::unix::fs::symlink(fx.agent.join("auth.json"), &link).unwrap();
        assert!(
            denied(read(&fx, &loaded, &link)),
            "the file resolves outside the skill folder"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_project_skill_folder_linked_to_the_agent_dir_opens_nothing() {
    let fx = fixture();
    write(&fx.agent.join("auth.json"), "secret");
    std::fs::create_dir_all(fx.cwd.join(".agents")).unwrap();
    std::os::unix::fs::symlink(&fx.agent, fx.cwd.join(".agents/skills")).unwrap();
    let loaded = nested(&fx);
    assert!(denied(read(
        &fx,
        &loaded,
        &fx.cwd.join(".agents/skills/auth.json")
    )));
}

#[cfg(unix)]
#[test]
fn a_user_skill_folder_linked_to_home_opens_nothing_and_one_linked_to_a_skill_repo_reads() {
    let fx = fixture();
    write(&fx.paths.home.join(".ssh/id"), "secret");
    std::fs::create_dir_all(fx.paths.home.join(".agents")).unwrap();
    let alias = fx.paths.home.join(".agents/skills");
    std::os::unix::fs::symlink(&fx.paths.home, &alias).unwrap();
    assert!(
        denied(read(&fx, &nested(&fx), &alias.join(".ssh/id"))),
        "the alias holds the home dir"
    );
    std::fs::remove_file(&alias).unwrap();
    let repo = fx.paths.home.parent().unwrap().join("dotfiles/skills");
    write(&repo.join("w/SKILL.md"), "skill");
    std::os::unix::fs::symlink(&repo, &alias).unwrap();
    assert_eq!(
        read(&fx, &nested(&fx), &alias.join("w/SKILL.md")),
        None,
        "a user skill folder kept elsewhere"
    );
}

#[cfg(unix)]
#[test]
fn a_user_skill_alias_to_a_file_is_not_a_skill_folder() {
    let fx = fixture();
    write(&fx.agent.join("auth.json"), "secret");
    std::fs::create_dir_all(fx.paths.home.join(".agents")).unwrap();
    let alias = fx.paths.home.join(".agents/skills");
    std::os::unix::fs::symlink(fx.agent.join("auth.json"), &alias).unwrap();
    assert!(
        denied(read(&fx, &nested(&fx), &alias)),
        "only a directory is a skill folder"
    );
}
