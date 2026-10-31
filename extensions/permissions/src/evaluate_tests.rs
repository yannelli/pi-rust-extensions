use super::*;
use crate::codex::parse_rules;
use crate::rules::Source;

fn claude(source: Source, decision: Decision, text: &str) -> Rule {
    let (tool, spec) = split_claude(text).unwrap();
    Rule {
        source,
        decision,
        text: text.into(),
        file: PathBuf::from("/f"),
        justification: None,
        kind: Kind::Claude(ClaudeRule {
            tool,
            spec,
            anchor: PathBuf::from("/w"),
        }),
    }
}

fn user(decision: Decision, text: &str) -> Rule {
    claude(Source::ClaudeUser, decision, text)
}

fn codex(text: &str) -> Vec<Rule> {
    parse_rules(text)
        .unwrap()
        .rules
        .into_iter()
        .map(|r| Rule {
            source: Source::CodexUser,
            decision: r.decision,
            text: r.text,
            file: PathBuf::from("/c.rules"),
            justification: r.justification,
            kind: Kind::Codex(r.rule),
        })
        .collect()
}

fn decide(rules: &[Rule], tool: &str, input: Value) -> Option<Decision> {
    let call = Call {
        tool,
        input: &input,
        cwd: Path::new("/w"),
        home: Path::new("/h"),
        dirs: &[PathBuf::from("/w")],
    };
    evaluate(rules, &call).decision
}

fn bash(rules: &[Rule], command: &str) -> Option<Decision> {
    decide(rules, "bash", json!({ "command": command }))
}

#[test]
fn compound_commands() {
    let rules = [
        user(Decision::Allow, "Bash(npm *)"),
        user(Decision::Allow, "Bash(git status)"),
        user(Decision::Deny, "Bash(git push *)"),
        user(Decision::Ask, "Bash(git clean *)"),
    ];
    assert_eq!(bash(&rules, "npm test"), Some(Decision::Allow));
    assert_eq!(
        bash(&rules, "git status && npm test | npm run x; npm ci"),
        Some(Decision::Allow)
    );
    assert_eq!(bash(&rules, "npm test && curl x"), None);
    assert_eq!(
        bash(&rules, "npm test && git push origin"),
        Some(Decision::Deny)
    );
    assert_eq!(bash(&rules, "ls | git push"), Some(Decision::Deny));
    assert_eq!(bash(&rules, "cd /tmp; git clean -f"), Some(Decision::Ask));
    assert_eq!(
        bash(&rules, r#"echo "$(git clean -f)""#),
        Some(Decision::Ask)
    );
    assert_eq!(
        bash(&rules, "git clean -f && git push"),
        Some(Decision::Deny)
    );
    assert_eq!(bash(&rules, "npm test &&"), None);
    assert_eq!(bash(&rules, "timeout 5 npm test"), Some(Decision::Allow));
    assert_eq!(bash(&rules, "FOO=1 git push x"), Some(Decision::Deny));
    assert_eq!(bash(&rules, "FOO=1 npm test"), None);
}

#[test]
fn precedence() {
    let rules = [
        user(Decision::Allow, "Bash(aws s3 ls)"),
        user(Decision::Deny, "Bash(aws *)"),
    ];
    assert_eq!(bash(&rules, "aws s3 ls"), Some(Decision::Deny));
    let rules = [
        user(Decision::Allow, "Bash(git *)"),
        user(Decision::Ask, "Bash(git push *)"),
    ];
    assert_eq!(bash(&rules, "git push"), Some(Decision::Ask));
    let pi_allow = claude(Source::PiGlobal, Decision::Allow, "Bash(git push)");
    let rules = [user(Decision::Ask, "Bash(git push *)"), pi_allow.clone()];
    assert_eq!(
        bash(&rules, "git push"),
        Some(Decision::Ask),
        "ask wins in every scope"
    );
    let rules = [
        user(Decision::Allow, "Bash(git push)"),
        claude(Source::PiProject, Decision::Ask, "Bash(git push)"),
    ];
    assert_eq!(bash(&rules, "git push"), Some(Decision::Ask));
    let rules = [user(Decision::Deny, "Bash(git push *)"), pi_allow];
    assert_eq!(bash(&rules, "git push"), Some(Decision::Deny));
    let rules = [
        claude(Source::PiProject, Decision::Ask, "Bash(git *)"),
        claude(Source::PiGlobal, Decision::Allow, "Bash(git push)"),
    ];
    assert_eq!(bash(&rules, "git push"), Some(Decision::Ask));
}

#[test]
fn file_rules() {
    let rules = [
        user(Decision::Deny, "Read(./.env)"),
        user(Decision::Allow, "Edit(src/**)"),
        user(Decision::Ask, "Edit(secrets/**)"),
        user(Decision::Allow, "Write(docs/**)"),
    ];
    assert_eq!(
        decide(&rules, "read", json!({ "path": ".env" })),
        Some(Decision::Deny)
    );
    assert_eq!(
        decide(&rules, "write", json!({ "path": "/w/.env" })),
        Some(Decision::Deny)
    );
    assert_eq!(
        decide(&rules, "grep", json!({ "pattern": "x", "path": ".env" })),
        Some(Decision::Deny)
    );
    assert_eq!(
        decide(&rules, "edit", json!({ "path": "src/a.rs" })),
        Some(Decision::Allow)
    );
    assert_eq!(
        decide(&rules, "edit", json!({ "path": "lib/src/a.rs" })),
        None
    );
    assert_eq!(
        decide(&rules, "write", json!({ "path": "lib/secrets/k" })),
        Some(Decision::Ask)
    );
    assert_eq!(
        decide(&rules, "write", json!({ "path": "docs/a.md" })),
        None
    );
    assert_eq!(decide(&rules, "read", json!({ "path": "src/a.rs" })), None);
}

#[test]
fn tool_names_domains_and_mcp() {
    let rules = [
        user(Decision::Allow, "WebFetch(domain:*.example.com)"),
        user(Decision::Deny, "WebFetch(domain:evil.com)"),
        user(Decision::Allow, "mcp__docs"),
        user(Decision::Deny, "mcp__fs__write"),
        user(Decision::Allow, "mcp__*"),
        user(Decision::Ask, "WebSearch"),
        user(Decision::Deny, "Agent(model:opus)"),
    ];
    let fetch = |urls: Value| decide(&rules, "url_context", json!({ "urls": urls }));
    assert_eq!(
        fetch(json!(["https://a.example.com/x"])),
        Some(Decision::Allow)
    );
    assert_eq!(
        fetch(json!(["https://a.example.com", "http://other.org"])),
        None
    );
    assert_eq!(
        fetch(json!(["https://a.example.com", "https://evil.com"])),
        Some(Decision::Deny)
    );
    assert_eq!(
        decide(&rules, "mcp__docs__search", json!({})),
        Some(Decision::Allow)
    );
    assert_eq!(
        decide(&rules, "mcp__fs__write", json!({})),
        Some(Decision::Deny)
    );
    assert_eq!(decide(&rules, "mcp__fs__read", json!({})), None);
    assert_eq!(
        decide(&rules, "web_search", json!({ "query": "x" })),
        Some(Decision::Ask)
    );
    assert_eq!(
        decide(&rules, "Agent", json!({ "model": "opus" })),
        Some(Decision::Deny)
    );
    assert_eq!(decide(&rules, "Agent", json!({ "model": "haiku" })), None);
    let star = [user(Decision::Deny, "*")];
    assert_eq!(decide(&star, "todo", json!({})), Some(Decision::Deny));
}

#[test]
fn codex_prefix_rules() {
    let rules = codex(
        r#"prefix_rule(pattern=["git", ["push", "reset"]], decision="forbidden", justification="no")
prefix_rule(pattern=["npm"], decision="allow")
prefix_rule(pattern=["curl"], decision="prompt")"#,
    );
    assert_eq!(bash(&rules, "git push origin"), Some(Decision::Deny));
    assert_eq!(bash(&rules, "npm test && npm ci"), Some(Decision::Allow));
    assert_eq!(bash(&rules, "npm test | curl x"), Some(Decision::Ask));
    assert_eq!(bash(&rules, "npm test > out"), None);
    assert_eq!(
        bash(&rules, "echo $(git reset --hard)"),
        Some(Decision::Deny)
    );
    assert_eq!(bash(&rules, "npm 'test'"), Some(Decision::Allow));
}

#[test]
fn suggestions_and_samples() {
    let rules = [
        user(Decision::Allow, "Bash(npm *)"),
        user(Decision::Ask, "Bash(git *)"),
    ];
    let input = json!({ "command": "npm ci && git push && timeout 3 ls -la" });
    let call = Call {
        tool: "bash",
        input: &input,
        cwd: Path::new("/w"),
        home: Path::new("/h"),
        dirs: &[PathBuf::from("/w")],
    };
    let verdict = evaluate(&rules, &call);
    assert_eq!(
        suggestions(&verdict, "bash"),
        ["Bash(git push)", "Bash(ls -la)"]
    );
    let input = json!({ "path": "a.txt" });
    let call = Call {
        tool: "write",
        input: &input,
        cwd: Path::new("/w"),
        home: Path::new("/h"),
        dirs: &[PathBuf::from("/w")],
    };
    assert_eq!(
        suggestions(&evaluate(&[], &call), "write"),
        ["Edit(//w/a.txt)"]
    );
    assert_eq!(
        sample_call("git push origin"),
        ("bash".into(), json!({ "command": "git push origin" }))
    );
    assert_eq!(
        sample_call("Read(./.env)"),
        ("read".into(), json!({ "path": "./.env" }))
    );
    assert_eq!(
        sample_call("mcp__fs__read"),
        ("mcp__fs__read".into(), json!({}))
    );
    assert_eq!(
        sample_call("ls"),
        ("bash".into(), json!({ "command": "ls" }))
    );
}

fn at(rules: &[Rule], tool: &str, input: Value, cwd: &Path) -> Option<Decision> {
    let dirs = [cwd.to_path_buf()];
    let call = Call {
        tool,
        input: &input,
        cwd,
        home: Path::new("/h"),
        dirs: &dirs,
    };
    evaluate(rules, &call).decision
}

#[test]
fn read_and_edit_rules_cover_bash_file_commands_and_redirects() {
    let rules = [
        user(Decision::Deny, "Read(./.env)"),
        user(Decision::Ask, "Edit(./config/**)"),
        user(Decision::Allow, "Bash(cat *)"),
        user(Decision::Allow, "Bash(echo *)"),
        user(Decision::Allow, "Bash(sed *)"),
    ];
    assert_eq!(bash(&rules, "cat .env"), Some(Decision::Deny));
    assert_eq!(bash(&rules, "cd sub && cat ../.env"), Some(Decision::Deny));
    assert_eq!(bash(&rules, "echo x < .env"), Some(Decision::Deny));
    assert_eq!(
        bash(&rules, "echo x > .env"),
        Some(Decision::Deny),
        "Read deny blocks writes"
    );
    assert_eq!(
        bash(&rules, "echo x > config/app.toml"),
        Some(Decision::Ask)
    );
    assert_eq!(
        bash(&rules, "sed -i s/a/b/ config/app.toml"),
        Some(Decision::Ask)
    );
    assert_eq!(
        bash(&rules, "sed -n 1p config/app.toml"),
        Some(Decision::Allow)
    );
    assert_eq!(
        bash(&rules, "cat notes.txt > out.txt"),
        Some(Decision::Allow)
    );
    assert_eq!(
        bash(&rules, "cat notes.txt > /tmp/out.txt"),
        None,
        "outside the working dirs"
    );
    assert_eq!(
        bash(&rules, "cat <<EOF\ncat .env\nEOF"),
        Some(Decision::Allow)
    );
}

#[test]
fn heredoc_bodies_do_not_match_rules() {
    let rules = [
        user(Decision::Deny, "Bash(rm *)"),
        user(Decision::Allow, "Bash(cat *)"),
    ];
    assert_eq!(
        bash(&rules, "cat <<'EOF' > script.sh\nrm -rf /\nEOF"),
        Some(Decision::Allow)
    );
    assert_eq!(
        bash(&rules, "cat <<EOF\n$(rm -rf x)\nEOF"),
        Some(Decision::Deny)
    );
}

#[test]
fn wildcard_allow_rules_do_not_approve_exec_wrappers_or_find_actions() {
    let rules = [
        user(Decision::Allow, "Bash(find *)"),
        user(Decision::Allow, "Bash(watch *)"),
        user(Decision::Allow, "Bash(find build -type f -delete)"),
    ];
    assert_eq!(bash(&rules, "find . -name x"), Some(Decision::Allow));
    assert_eq!(bash(&rules, "find . -name x -delete"), None);
    assert_eq!(bash(&rules, "find . -exec rm {} ;"), None);
    assert_eq!(bash(&rules, "watch ls"), None);
    assert_eq!(
        bash(&rules, "find build -type f -delete"),
        Some(Decision::Allow)
    );
}

#[test]
fn negation_carves_out_of_earlier_relative_rules_in_the_same_list() {
    let rules = [
        user(Decision::Deny, "Read(*.env)"),
        user(Decision::Deny, "Read(!sample.env)"),
        user(Decision::Deny, "Read(secrets/**)"),
        user(Decision::Deny, "Read(!secrets/public/**)"),
    ];
    let read = |path: &str| decide(&rules, "read", json!({ "path": path }));
    assert_eq!(read("prod.env"), Some(Decision::Deny));
    assert_eq!(read("a/sample.env"), None);
    assert_eq!(read("secrets/public/x.txt"), Some(Decision::Deny));
    let mut other_file = claude(Source::ClaudeProject, Decision::Deny, "Read(!prod.env)");
    other_file.file = PathBuf::from("/w/.claude/settings.json");
    let rules = [user(Decision::Deny, "Read(*.env)"), other_file];
    assert_eq!(
        decide(&rules, "read", json!({ "path": "prod.env" })),
        Some(Decision::Deny)
    );
}

#[cfg(unix)]
#[test]
fn symlinks_match_deny_on_either_path_and_allow_on_both() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("work");
    let secret = dir.path().join("secret");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&secret).unwrap();
    std::fs::write(secret.join("key"), "k").unwrap();
    std::os::unix::fs::symlink(secret.join("key"), cwd.join("link")).unwrap();
    let deny = format!("Read(/{}/**)", secret.display());
    let rules = [user(Decision::Deny, &deny)];
    assert_eq!(
        at(&rules, "read", json!({ "path": "link" }), &cwd),
        Some(Decision::Deny)
    );
    let rules = [user(Decision::Allow, "Read(./**)")];
    assert_eq!(at(&rules, "read", json!({ "path": "link" }), &cwd), None);
    std::fs::write(cwd.join("plain.txt"), "p").unwrap();
    assert_eq!(
        at(&rules, "read", json!({ "path": "plain.txt" }), &cwd),
        Some(Decision::Allow)
    );
}

#[test]
fn agent_rules_match_subagent_calls() {
    let rules = [user(Decision::Deny, "Agent(Explore)")];
    assert_eq!(
        decide(&rules, "subagent", json!({ "task": "x" })),
        Some(Decision::Deny)
    );
    assert_eq!(
        decide(&rules, "subagent", json!({ "task": "x", "agent": "build" })),
        None
    );
    let tasks = json!({ "tasks": [{ "task": "a", "agent": "build" }, { "task": "b" }] });
    assert_eq!(decide(&rules, "subagent", tasks), Some(Decision::Deny));
}

#[test]
fn codex_rules_see_unsplittable_scripts_and_absolute_programs() {
    let rules = codex(
        r#"
prefix_rule(pattern=["bash", "-lc"], decision="prompt")
prefix_rule(pattern=["git", "push"], decision="forbidden")
host_executable(name="git", paths=["/opt/homebrew/bin/git"])
"#,
    );
    assert_eq!(bash(&rules, "echo $HOME > out.txt"), Some(Decision::Ask));
    assert_eq!(bash(&rules, "git status && ls"), None);
    assert_eq!(
        bash(&rules, "/usr/bin/git push origin"),
        Some(Decision::Deny)
    );
    let mut constrained = rules.clone();
    for rule in &mut constrained {
        if let Kind::Codex(prefix) = &mut rule.kind
            && prefix.pattern[0][0] == "git"
        {
            prefix.hosts = Some(vec!["/opt/homebrew/bin/git".into()]);
        }
    }
    assert_eq!(bash(&constrained, "/usr/bin/git push origin"), None);
    assert_eq!(
        bash(&constrained, "/opt/homebrew/bin/git push"),
        Some(Decision::Deny)
    );
}

#[test]
fn codex_network_rules_decide_fetch_hosts() {
    let parsed =
        parse_rules(r#"network_rule(host="Evil.example", protocol="https", decision="deny")"#)
            .unwrap();
    let rules: Vec<Rule> = parsed
        .network
        .into_iter()
        .map(|(rule, decision, justification, text)| Rule {
            source: Source::CodexUser,
            decision,
            text,
            file: PathBuf::from("/c.rules"),
            justification,
            kind: Kind::Network(rule),
        })
        .collect();
    let fetch = |url: &str| decide(&rules, "fetch", json!({ "url": url }));
    assert_eq!(fetch("https://evil.example/x"), Some(Decision::Deny));
    assert_eq!(fetch("https://good.example/x"), None);
    assert_eq!(
        bash(&rules, "curl https://evil.example"),
        None,
        "bash network needs a sandbox"
    );
}
