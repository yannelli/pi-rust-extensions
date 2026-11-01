//! How a Codex rules file with an error changes what loads, as in Codex `load_exec_policy_with_warning`.

use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    paths: Paths,
    agent: PathBuf,
    cwd: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cwd = root.join("project");
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    let paths = Paths {
        home: root.join("home"),
        claude_dir: root.join("home/.claude"),
        claude_managed: root.join("managed/managed-settings.json"),
        codex_home: root.join("home/.codex"),
        codex_requirements: vec![root.join("etc/codex/requirements.toml")],
        agent_dir: Some(root.join("agent")),
    };
    Fixture {
        agent: root.join("agent"),
        cwd,
        paths,
        _dir: dir,
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

impl Fixture {
    fn load(&self, trusted: bool) -> Loaded {
        let env = Env {
            paths: &self.paths,
            agent_dir: self.agent.clone(),
            cwd: self.cwd.clone(),
            trusted,
        };
        load(&env)
    }

    fn user_rules(&self, name: &str, text: &str) {
        write(&self.paths.codex_home.join("rules").join(name), text);
    }

    /// Every source kind: Codex user, project, and managed; Claude user; pi global.
    fn all_sources(&self) {
        self.user_rules(
            "default.rules",
            "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")\n\
             host_executable(name=\"git\", paths=[\"/opt/git\"])",
        );
        write(
            &self.cwd.join(".codex/rules/p.rules"),
            "prefix_rule(pattern=[\"curl\"], decision=\"forbidden\")",
        );
        write(
            &self.paths.codex_requirements[0],
            "[rules]\nprefix_rules = [{ pattern = [{ token = \"git\" }, { token = \"push\" }], decision = \"forbidden\" }]\n",
        );
        write(
            &self.paths.claude_dir.join("settings.json"),
            r#"{"permissions":{"deny":["Bash(echo *)"]}}"#,
        );
        write(
            &self.agent.join("ext/permissions.json"),
            r#"{"deny":["Bash(touch *)"]}"#,
        );
    }
}

fn sources(loaded: &Loaded) -> Vec<Source> {
    loaded.rules.iter().map(|rule| rule.source).collect()
}

fn managed_hosts(loaded: &Loaded) -> Option<Vec<String>> {
    loaded
        .rules
        .iter()
        .find_map(|rule| match (&rule.kind, rule.source) {
            (Kind::Codex(prefix), Source::CodexManaged) => Some(prefix.hosts.clone()),
            _ => None,
        })?
}

#[test]
fn valid_codex_files_load_every_source_and_their_host_executables() {
    let fx = fixture();
    fx.all_sources();
    let loaded = fx.load(true);
    for source in [
        Source::CodexUser,
        Source::CodexProject,
        Source::CodexManaged,
        Source::ClaudeUser,
        Source::PiGlobal,
    ] {
        assert!(
            sources(&loaded).contains(&source),
            "{source:?} missing: {:?}",
            loaded.notes
        );
    }
    assert_eq!(managed_hosts(&loaded), Some(vec!["/opt/git".to_string()]));
}

#[test]
fn one_bad_user_or_project_file_drops_all_codex_user_and_project_rules_and_mappings() {
    for (bad_file, bad_text, error) in [
        (
            "user",
            r#"network_rule(host="*.evil.example", protocol="https", decision="deny")"#,
            "wildcards are not allowed",
        ),
        (
            "user",
            r#"network_rule(host="evil.example", protocol="ftp", decision="deny")"#,
            "(got ftp)",
        ),
        (
            "project",
            r#"network_rule(host="https://evil.example", protocol="https", decision="deny")"#,
            "without scheme or path",
        ),
    ] {
        let fx = fixture();
        fx.all_sources();
        let file = match bad_file {
            "user" => fx.paths.codex_home.join("rules/zz-bad.rules"),
            _ => fx.cwd.join(".codex/rules/zz-bad.rules"),
        };
        write(&file, bad_text);
        let loaded = fx.load(true);
        let found = sources(&loaded);
        assert!(
            !found.contains(&Source::CodexUser) && !found.contains(&Source::CodexProject),
            "{bad_file}: {found:?}"
        );
        for kept in [Source::CodexManaged, Source::ClaudeUser, Source::PiGlobal] {
            assert!(found.contains(&kept), "{bad_file}: {kept:?} was dropped");
        }
        assert_eq!(
            managed_hosts(&loaded),
            None,
            "{bad_file}: a dropped host_executable still applies"
        );
        let file_note = format!("{}: ", file.display());
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note.starts_with(&file_note) && note.contains(error)),
            "{bad_file}: {:?}",
            loaded.notes
        );
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note.starts_with("Codex user and project rules are not applied"))
        );
    }
}

#[test]
fn a_bad_file_before_valid_ones_still_drops_the_later_files() {
    let fx = fixture();
    fx.user_rules(
        "aa-bad.rules",
        r#"network_rule(host="a b", protocol="https", decision="deny")"#,
    );
    fx.user_rules(
        "zz-good.rules",
        "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")",
    );
    let loaded = fx.load(false);
    assert!(
        !sources(&loaded).contains(&Source::CodexUser),
        "{:?}",
        loaded.notes
    );
}

#[test]
fn a_bad_file_in_an_untrusted_project_is_not_read_and_user_rules_stay() {
    let fx = fixture();
    fx.user_rules(
        "default.rules",
        "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")",
    );
    write(
        &fx.cwd.join(".codex/rules/bad.rules"),
        r#"network_rule(host="*", protocol="https", decision="deny")"#,
    );
    let loaded = fx.load(false);
    assert!(
        sources(&loaded).contains(&Source::CodexUser),
        "{:?}",
        loaded.notes
    );
    assert!(
        !loaded
            .notes
            .iter()
            .any(|note| note.starts_with("Codex user and project rules are not applied"))
    );
}
