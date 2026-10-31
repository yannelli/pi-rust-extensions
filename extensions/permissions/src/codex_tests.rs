use super::*;

const RULES: &str = r#"
# Codex rules
prefix_rule(
    pattern = ["git", ["push", "fetch"]],
    decision = "prompt",
    justification = "network access",
    match = [["git", "push"], "git fetch origin"],
    not_match = ["git status"],
)
prefix_rule(pattern=[["rm", "rmdir"], "-rf"], decision="forbidden", justification='Use `trash` instead.')
prefix_rule(["ls"])
host_executable(name="git", paths=["/usr/bin/git"])
network_rule(host="example.com", protocol="https", decision="deny")
"#;

fn words(s: &str) -> Vec<String> {
    s.split(' ').map(str::to_owned).collect()
}

#[test]
fn parses_prefix_rules() {
    let parsed = parse_rules(RULES).unwrap();
    assert_eq!(parsed.skipped, ["host_executable", "network_rule"]);
    assert_eq!(parsed.rules.len(), 4);
    let push = &parsed.rules[0];
    assert_eq!(push.decision, Decision::Ask);
    assert_eq!(push.justification.as_deref(), Some("network access"));
    assert!(push.rule.matches(&words("git push origin main")));
    assert!(push.rule.matches(&words("git fetch")));
    assert_eq!(
        push.text,
        r#"prefix_rule(pattern=["git", ["push", "fetch"]], decision="prompt")"#
    );
    let rm: Vec<_> = parsed.rules[1..3].iter().collect();
    assert!(rm.iter().all(|r| r.decision == Decision::Deny));
    assert!(rm[0].rule.matches(&words("rm -rf /")));
    assert!(rm[1].rule.matches(&words("rmdir -rf x")));
    assert_eq!(rm[0].justification.as_deref(), Some("Use `trash` instead."));
    assert_eq!(parsed.rules[3].decision, Decision::Allow);
}

#[test]
fn rejects_bad_files() {
    assert!(parse_rules(r#"prefix_rule(pattern=[], decision="allow")"#).is_err());
    assert!(parse_rules(r#"prefix_rule(pattern=["a"], decision="maybe")"#).is_err());
    assert!(parse_rules(r#"x = 1"#).is_err());
    assert!(parse_rules(r#"prefix_rule(pattern=["a"], match=["b"])"#).is_err());
    assert!(parse_rules(r#"prefix_rule(pattern=["a"], not_match=["a b"])"#).is_err());
    assert!(parse_rules(r#"prefix_rule(pattern=["a""#).is_err());
    assert!(parse_rules(r#"exec_rule(pattern=["a"])"#).is_err());
}

#[test]
fn strings_and_triple_quotes() {
    let text = "prefix_rule(pattern=[r'a\\b', \"\"\"c\nd\"\"\"], justification=\"x\\\"y\")";
    let parsed = parse_rules(text).unwrap();
    assert_eq!(
        parsed.rules[0].rule.pattern,
        [vec!["a\\b".to_owned()], vec!["c\nd".to_owned()]]
    );
    assert_eq!(parsed.rules[0].justification.as_deref(), Some("x\"y"));
}
