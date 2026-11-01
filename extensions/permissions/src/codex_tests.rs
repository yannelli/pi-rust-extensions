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
    assert!(parsed.skipped.is_empty());
    assert_eq!(
        parsed.hosts,
        [("git".to_owned(), vec!["/usr/bin/git".to_owned()])]
    );
    let (network, decision, _, _) = &parsed.network[0];
    assert_eq!(
        (network.host.as_str(), network.protocol.as_str()),
        ("example.com", "https")
    );
    assert_eq!(*decision, Decision::Deny);
    assert!(parse_rules(r#"host_executable(name="git", paths=["/usr/bin/gitx"])"#).is_err());
    assert!(
        parse_rules(r#"network_rule(host="x.com", protocol="https", decision="maybe")"#).is_err()
    );
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

fn network(host: &str, protocol: &str) -> Result<(String, String), String> {
    let text = format!(r#"network_rule(host="{host}", protocol="{protocol}", decision="deny")"#);
    let parsed = parse_rules(&text)?;
    let (rule, ..) = parsed.network.into_iter().next().unwrap();
    Ok((rule.host, rule.protocol))
}

/// Cases from Codex execpolicy `rule.rs` and `parser.rs` at openai/codex 67f0994b.
#[test]
fn network_rules_follow_codex_protocols_and_host_normalization() {
    for (written, stored) in [
        ("http", "http"),
        ("https", "https"),
        ("https_connect", "https"),
        ("http-connect", "https"),
        ("socks5_tcp", "socks5_tcp"),
        ("socks5_udp", "socks5_udp"),
    ] {
        assert_eq!(
            network("a.example", written).unwrap().1,
            stored,
            "{written}"
        );
    }
    assert_eq!(
        network("a.example", "ftp").unwrap_err(),
        "network_rule protocol must be one of http, https, socks5_tcp, socks5_udp (got ftp)"
    );
    for (written, host) in [
        ("Example.COM.", "example.com"),
        ("  a.example  ", "a.example"),
        ("a.example:8080", "a.example"),
        ("[::1]", "::1"),
        ("[::1]:443", "::1"),
        ("::1", "::1"),
        ("10.0.0.1:80", "10.0.0.1"),
    ] {
        assert_eq!(network(written, "https").unwrap().0, host, "{written}");
    }
    for (written, error) in [
        ("", "network_rule host cannot be empty"),
        (".", "network_rule host cannot be empty"),
        (
            "https://a.example",
            "network_rule host must be a hostname or IP literal (without scheme or path)",
        ),
        (
            "a.example/p",
            "network_rule host must be a hostname or IP literal (without scheme or path)",
        ),
        (
            "a.example?q",
            "network_rule host must be a hostname or IP literal (without scheme or path)",
        ),
        (
            "[::1",
            "network_rule host has an invalid bracketed IPv6 literal",
        ),
        (
            "[::1]x",
            "network_rule host contains an unsupported suffix: [::1]x",
        ),
        (
            "[::1]:",
            "network_rule host contains an unsupported suffix: [::1]:",
        ),
        (
            "*.a.example",
            "network_rule host must be a specific host; wildcards are not allowed",
        ),
        (
            "a.*",
            "network_rule host must be a specific host; wildcards are not allowed",
        ),
        ("a b", "network_rule host cannot contain whitespace"),
    ] {
        assert_eq!(network(written, "https").unwrap_err(), error, "{written:?}");
    }
}

#[test]
fn a_bad_network_rule_rejects_the_whole_file_in_codex_order() {
    let protocol_first = r#"network_rule(host="*", protocol="ftp", decision="maybe")"#;
    assert!(
        parse_rules(protocol_first)
            .unwrap_err()
            .contains("protocol must be one of")
    );
    let decision_next = r#"network_rule(host="*", protocol="https", decision="maybe")"#;
    assert_eq!(
        parse_rules(decision_next).unwrap_err(),
        "invalid decision: maybe"
    );
    let justification =
        r#"network_rule(host="*", protocol="https", decision="deny", justification=" ")"#;
    assert_eq!(
        parse_rules(justification).unwrap_err(),
        "justification cannot be empty"
    );
    let file = "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")\n\
                network_rule(host=\"*.a.example\", protocol=\"https\", decision=\"deny\")\n";
    assert!(
        parse_rules(file).is_err(),
        "a valid rule before a bad one does not load"
    );
    let deny = parse_rules(r#"network_rule(host="a.example", protocol="https", decision="deny")"#);
    assert_eq!(deny.unwrap().network[0].1, Decision::Deny);
}

/// The Codex `network_rule(host, protocol, decision, justification)` Starlark signature takes
/// positional or named arguments.
#[test]
fn network_rules_take_positional_or_named_arguments() {
    let positional = parse_rules(r#"network_rule("Evil.example", "https", "deny")"#).unwrap();
    let (rule, decision, justification, _) = &positional.network[0];
    assert_eq!(
        (rule.host.as_str(), *decision, justification),
        ("evil.example", Decision::Deny, &None)
    );
    let mixed =
        parse_rules(r#"network_rule("a.example", "http", decision="allow", justification="ok")"#);
    let (rule, decision, justification, _) = &mixed.unwrap().network[0];
    assert_eq!(
        (rule.protocol.as_str(), *decision),
        ("http", Decision::Allow)
    );
    assert_eq!(justification.as_deref(), Some("ok"));
    for (text, error) in [
        (
            r#"network_rule("a", "https", "deny", host="b")"#,
            "network_rule got `host` twice",
        ),
        (
            r#"network_rule(host="a", protocol="https", decision="deny", reason="x")"#,
            "network_rule has no parameter `reason`",
        ),
        (
            r#"network_rule("a", "https", "deny", "j", "extra")"#,
            "network_rule takes at most 4 arguments",
        ),
        (
            r#"network_rule(host="a", decision="deny")"#,
            "network_rule needs `protocol`",
        ),
        (
            r#"network_rule(protocol="https", "evil.example", decision="forbidden")"#,
            "network_rule has a positional argument after a named one",
        ),
    ] {
        assert_eq!(parse_rules(text).unwrap_err(), error, "{text}");
    }
}
