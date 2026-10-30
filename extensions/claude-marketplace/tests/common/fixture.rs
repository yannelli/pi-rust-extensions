use std::path::{Path, PathBuf};
use std::process::Command;

use pi_ext_claude_marketplace::{Env, Paths};
use serde_json::{Value, json};
use tempfile::TempDir;

pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub fn write_json(path: &Path, value: &Value) {
    write(path, &serde_json::to_string_pretty(value).unwrap());
}

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read(path)).unwrap()
}

pub fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A temp layout: a git marketplace repo, an agent dir, a project, and a Claude config dir.
pub struct Fixture {
    _tmp: TempDir,
    pub repo: PathBuf,
    pub agent: PathBuf,
    pub project: PathBuf,
    pub home: PathBuf,
    pub claude: PathBuf,
}

const HOOKS: &str = r#"{ "hooks": { "PreToolUse": [ { "matcher": "Bash",
  "hooks": [ { "type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/scripts/check.sh ${UNKNOWN}", "timeout": 30 } ] } ] } }"#;

fn plugin_fmt(root: &Path) {
    write_json(
        &root.join(".claude-plugin/plugin.json"),
        &json!({ "name": "fmt", "version": "1.0.0" }),
    );
    write(
        &root.join("commands/hello.md"),
        "---\ndescription: Say hello\n---\nHello $ARGUMENTS\n",
    );
    write(
        &root.join("commands/build/web.md"),
        "Build with ${CLAUDE_PLUGIN_ROOT}/web\n",
    );
    write(
        &root.join("skills/check/SKILL.md"),
        "---\nname: check\ndescription: Check code\n---\nRun ${CLAUDE_SKILL_DIR}/run.sh\n",
    );
    write(&root.join("skills/check/ref.txt"), "ref\n");
    write(
        &root.join("skills/fmt-style/SKILL.md"),
        "---\nname: style\ndescription: Style\n---\nStyle\n",
    );
    write(
        &root.join("skills/.hidden/SKILL.md"),
        "---\nname: hidden\n---\n",
    );
    write(
        &root.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews\ntools: Bash, Read, Glob, WebFetch\n---\nReview in ${CLAUDE_PROJECT_DIR}\n",
    );
    write(&root.join("hooks/hooks.json"), HOOKS);
    write_json(
        &root.join(".mcp.json"),
        &json!({ "mcpServers": { "db": {
        "command": "${CLAUDE_PLUGIN_ROOT}/bin/db", "args": ["--data", "${CLAUDE_PLUGIN_DATA}"] } } }),
    );
}

fn plugin_lint(root: &Path) {
    write_json(
        &root.join("plugin.json"),
        &json!({ "name": "lint", "commands": "./extra", "lspServers": {} }),
    );
    write(&root.join("extra/run.md"), "Run lint\n");
    write(
        &root.join("skills/rules/SKILL.md"),
        "---\ndescription: Rules\n---\nRules\n",
    );
    write(
        &root.join("agents/linter.md"),
        "---\nname: linter\ntools:\n  - Grep\n  - LS\n---\nLint\n",
    );
    write(
        &root.join("hooks/hooks.json"),
        r#"{ "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "echo ${CLAUDE_PLUGIN_DATA}" } ] } ] } }"#,
    );
    write_json(
        &root.join(".mcp.json"),
        &json!({ "srv": { "url": "http://127.0.0.1:1/mcp" } }),
    );
}

impl Fixture {
    pub fn new() -> Fixture {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let repo = base.join("market");
        let catalog = json!({ "name": "acme", "owner": { "name": "Acme" }, "plugins": [
            { "name": "fmt", "source": "./plugins/fmt", "description": "Formatter" },
            { "name": "lint", "source": "./plugins/lint", "version": "0.1.0" },
            { "name": "pkg", "source": { "source": "npm", "package": "x" } },
            { "name": "escape", "source": "../outside" }
        ]});
        write_json(&repo.join(".claude-plugin/marketplace.json"), &catalog);
        plugin_fmt(&repo.join("plugins/fmt"));
        plugin_lint(&repo.join("plugins/lint"));
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let dirs = ["agent", "project", "home", "home/.claude"].map(|d| base.join(d));
        for dir in &dirs {
            std::fs::create_dir_all(dir).unwrap();
        }
        let [agent, project, home, claude] = dirs;
        Fixture {
            _tmp: tmp,
            repo,
            agent,
            project,
            home,
            claude,
        }
    }

    pub fn paths(&self) -> Paths {
        Paths {
            agent_dir: Some(self.agent.clone()),
            home: Some(self.home.clone()),
            claude_config_dir: Some(self.claude.clone()),
        }
    }

    pub fn env(&self, trusted: bool) -> Env {
        Env::new(
            &self.paths(),
            self.agent.clone(),
            self.project.clone(),
            trusted,
        )
    }

    pub fn commit(&self, message: &str) {
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "-q", "-m", message]);
    }

    pub fn file_url(&self) -> String {
        format!("file://{}", self.repo.display())
    }
}
