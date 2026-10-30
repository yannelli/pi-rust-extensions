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
    assert_eq!(bash(&rules, "git push"), Some(Decision::Allow));
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
