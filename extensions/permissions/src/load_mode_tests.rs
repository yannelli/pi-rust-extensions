use super::*;
use serde_json::json;

struct Fixture {
    _temp: tempfile::TempDir,
    paths: Paths,
    cwd: PathBuf,
    agent: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let cwd = root.join("project");
        std::fs::create_dir_all(cwd.join(".git")).unwrap();
        let paths = Paths {
            home: root.join("home"),
            claude_dir: root.join("home/.claude"),
            claude_managed: root.join("managed/settings.json"),
            codex_home: root.join("home/.codex"),
            codex_system: root.join("etc/codex"),
            codex_requirements: vec![root.join("managed/requirements.toml")],
            agent_dir: Some(root.join("agent")),
        };
        let agent = root.join("agent");
        Self {
            _temp: temp,
            paths,
            cwd,
            agent,
        }
    }

    fn file(&self, source: Source) -> PathBuf {
        match source {
            Source::ClaudeManaged => self.paths.claude_managed.clone(),
            Source::ClaudeUser => self.paths.claude_dir.join("settings.json"),
            Source::ClaudeProject => self.cwd.join(".claude/settings.json"),
            Source::ClaudeLocal => self.cwd.join(".claude/settings.local.json"),
            _ => unreachable!(),
        }
    }

    fn settings(&self, source: Source, permissions: Value) {
        let file = self.file(source);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, json!({ "permissions": permissions }).to_string()).unwrap();
    }

    fn mode(&self, source: Source, name: &str) {
        self.settings(source, json!({ "defaultMode": name }));
    }

    fn load(&self, trusted: bool) -> Loaded {
        load(&Env {
            paths: &self.paths,
            agent_dir: self.agent.clone(),
            cwd: self.cwd.clone(),
            trusted,
        })
    }
}

#[test]
fn project_and_local_restricted_modes_do_not_fall_back_to_the_user_mode() {
    for source in [Source::ClaudeProject, Source::ClaudeLocal] {
        for name in ["auto", "bypassPermissions"] {
            let fx = Fixture::new();
            fx.mode(Source::ClaudeUser, "acceptEdits");
            fx.mode(source, name);
            let loaded = fx.load(true);
            assert_eq!(
                loaded.claude_mode,
                Some(("default".into(), source)),
                "{source:?}: {name}"
            );
            assert_eq!(
                loaded.mode.as_ref().map(|(mode, _)| *mode),
                Some(Mode::Default)
            );
            let prefix = format!(
                "{}: defaultMode {name} is not applied; using default (Manual) mode",
                fx.file(source).display()
            );
            assert!(
                loaded.notes.iter().any(|note| note.starts_with(&prefix)),
                "{:?}",
                loaded.notes
            );
        }
    }
}

#[test]
fn restricted_project_modes_select_manual_without_a_user_mode() {
    for source in [Source::ClaudeProject, Source::ClaudeLocal] {
        for name in ["auto", "bypassPermissions"] {
            let fx = Fixture::new();
            fx.mode(source, name);
            let loaded = fx.load(true);
            assert_eq!(loaded.claude_mode, Some(("default".into(), source)));
            assert_eq!(
                loaded.mode.as_ref().map(|(mode, _)| *mode),
                Some(Mode::Default)
            );
        }
    }
}

#[test]
fn user_and_managed_can_select_auto_or_bypass() {
    for source in [Source::ClaudeUser, Source::ClaudeManaged] {
        for name in ["auto", "bypassPermissions"] {
            let fx = Fixture::new();
            fx.mode(source, name);
            let loaded = fx.load(true);
            assert_eq!(loaded.claude_mode, Some((name.into(), source)));
            assert_eq!(
                loaded.mode.as_ref().map(|(mode, _)| *mode),
                Mode::parse(name)
            );
            assert!(
                !loaded
                    .notes
                    .iter()
                    .any(|note| note.contains("is not applied;"))
            );
        }
    }
}

#[test]
fn other_project_modes_keep_their_settings_precedence() {
    for source in [Source::ClaudeProject, Source::ClaudeLocal] {
        for name in ["default", "manual", "acceptEdits", "plan", "dontAsk"] {
            let fx = Fixture::new();
            fx.mode(Source::ClaudeUser, "bypassPermissions");
            fx.mode(source, name);
            let loaded = fx.load(true);
            assert_eq!(loaded.claude_mode, Some((name.into(), source)));
            assert_eq!(
                loaded.mode.as_ref().map(|(mode, _)| *mode),
                Mode::parse(name)
            );
        }
    }
}

#[test]
fn ignored_mode_keeps_the_same_files_rules_and_additional_directories() {
    let fx = Fixture::new();
    fx.settings(
        Source::ClaudeProject,
        json!({
            "defaultMode": "bypassPermissions",
            "deny": ["Read(./.env)"],
            "additionalDirectories": ["../shared"]
        }),
    );
    let loaded = fx.load(true);
    assert_eq!(
        loaded.claude_mode,
        Some(("default".into(), Source::ClaudeProject))
    );
    assert!(
        loaded.rules.iter().any(|rule| {
            rule.source == Source::ClaudeProject && rule.decision == Decision::Deny
        })
    );
    assert!(
        loaded
            .dirs
            .contains(&crate::pattern::normalize(&fx.cwd.join("../shared")))
    );
}

#[test]
fn managed_precedence_and_project_trust_still_apply() {
    let fx = Fixture::new();
    fx.mode(Source::ClaudeManaged, "dontAsk");
    fx.mode(Source::ClaudeUser, "bypassPermissions");
    fx.mode(Source::ClaudeProject, "acceptEdits");
    fx.mode(Source::ClaudeLocal, "plan");
    let loaded = fx.load(true);
    assert_eq!(
        loaded.claude_mode,
        Some(("dontAsk".into(), Source::ClaudeManaged))
    );

    let untrusted = Fixture::new();
    untrusted.mode(Source::ClaudeUser, "default");
    untrusted.mode(Source::ClaudeProject, "acceptEdits");
    untrusted.mode(Source::ClaudeLocal, "bypassPermissions");
    let loaded = untrusted.load(false);
    assert_eq!(
        loaded.claude_mode,
        Some(("default".into(), Source::ClaudeUser))
    );
}

#[test]
fn a_restricted_local_mode_suppresses_lower_project_and_user_modes() {
    for name in ["auto", "bypassPermissions"] {
        let fx = Fixture::new();
        fx.mode(Source::ClaudeUser, "bypassPermissions");
        fx.mode(Source::ClaudeProject, "acceptEdits");
        fx.mode(Source::ClaudeLocal, name);
        let loaded = fx.load(true);
        assert_eq!(
            loaded.claude_mode,
            Some(("default".into(), Source::ClaudeLocal))
        );
        assert_eq!(
            loaded.mode.as_ref().map(|(mode, _)| *mode),
            Some(Mode::Default)
        );
    }
}

#[test]
fn managed_mode_wins_over_project_fallback_and_pi_can_still_opt_out() {
    let fx = Fixture::new();
    fx.mode(Source::ClaudeManaged, "auto");
    fx.mode(Source::ClaudeProject, "bypassPermissions");
    let loaded = fx.load(true);
    assert_eq!(
        loaded.claude_mode,
        Some(("auto".into(), Source::ClaudeManaged))
    );

    let fx = Fixture::new();
    fx.mode(Source::ClaudeProject, "auto");
    let global = global_file(&fx.agent);
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::write(global, json!({"defaultMode": "pi"}).to_string()).unwrap();
    let loaded = fx.load(true);
    assert_eq!(
        loaded.claude_mode,
        Some(("default".into(), Source::ClaudeProject))
    );
    assert_eq!(loaded.mode, None);
}
