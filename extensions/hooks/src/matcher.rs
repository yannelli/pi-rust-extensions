use regex::Regex;
use serde_json::Value;

use crate::model::Source;
use crate::tools::{codex_aliases, primary_input};

/// The value a matcher is tested against.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Events without a matcher: every group matches.
    Any,
    /// A tool event; the Claude tool name.
    Tool(String),
    Value(String),
}

pub fn matches(source: Source, matcher: Option<&str>, target: &Target) -> bool {
    let matcher = matcher.unwrap_or("").trim();
    if matcher.is_empty() || matcher == "*" {
        return true;
    }
    let candidates: Vec<&str> = match target {
        Target::Any => return true,
        Target::Tool(name) if source.is_codex() => {
            let mut names = vec![name.as_str()];
            names.extend(codex_aliases(name));
            names
        }
        Target::Tool(name) => vec![name.as_str()],
        Target::Value(value) => vec![value.as_str()],
    };
    if !source.is_codex() && is_name_list(matcher) {
        return matcher
            .split(['|', ','])
            .map(str::trim)
            .any(|name| candidates.contains(&name));
    }
    match Regex::new(matcher) {
        Ok(regex) => candidates.iter().any(|c| regex.is_match(c)),
        Err(err) => {
            pi_ext_sdk::warn!("hooks: invalid matcher {matcher:?}: {err}");
            false
        }
    }
}

fn is_name_list(matcher: &str) -> bool {
    matcher
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ' ' | ',' | '|'))
}

/// Claude `if`: one permission rule such as `Bash(git *)` or `Edit(*.ts)`.
pub fn if_rule_matches(rule: &str, tool: &str, input: &Value) -> bool {
    let rule = rule.trim();
    let (name, pattern) = match rule.split_once('(') {
        Some((name, rest)) => (name.trim(), rest.strip_suffix(')').map(str::trim)),
        None => (rule, None),
    };
    if name != tool {
        return false;
    }
    let Some(pattern) = pattern.filter(|p| !p.is_empty() && *p != "*") else {
        return true;
    };
    let pattern = pattern
        .strip_suffix(":*")
        .map_or(pattern.to_owned(), |p| format!("{p}*"));
    primary_input(tool, input).is_some_and(|value| glob(&pattern, value))
}

/// `*` matches any run of characters; everything else is literal; the whole value must match.
pub fn glob(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !value.starts_with(first) || value.len() < first.len() + last.len() || !value.ends_with(last)
    {
        return false;
    }
    let mut rest = &value[first.len()..value.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> Target {
        Target::Tool(name.into())
    }

    #[test]
    fn claude_matchers() {
        let c = Source::Claude;
        let cases = [
            (None, "Bash", true),
            (Some(""), "Bash", true),
            (Some("*"), "Bash", true),
            (Some("Bash"), "Bash", true),
            (Some("Bash"), "BashOutput", false),
            (Some("Edit|Write"), "Write", true),
            (Some("Edit, Write"), "Write", true),
            (Some("Edit|Write"), "Read", false),
            (Some("Edit.*"), "NotebookEdit", true),
            (Some("^Edit$"), "NotebookEdit", false),
            (Some("mcp__.*__write"), "mcp__fs__write", true),
            (Some("("), "Bash", false),
            (Some("Bash"), "shell", false),
        ];
        for (matcher, name, want) in cases {
            assert_eq!(
                matches(c, matcher, &tool(name)),
                want,
                "{matcher:?} vs {name}"
            );
        }
        assert!(matches(Source::Pi, Some("Bash|Read"), &tool("Read")));
        assert!(matches(
            c,
            Some("startup"),
            &Target::Value("startup".into())
        ));
        assert!(!matches(
            c,
            Some("startup"),
            &Target::Value("resume".into())
        ));
        assert!(matches(c, Some("Bash"), &Target::Any));
    }

    #[test]
    fn codex_matchers_are_regex_with_aliases() {
        let x = Source::Codex;
        assert!(
            matches(x, Some("Bash"), &tool("BashOutput")),
            "unanchored regex"
        );
        assert!(matches(x, Some("^shell$"), &tool("Bash")));
        assert!(matches(x, Some("exec_command"), &tool("Bash")));
        assert!(matches(x, Some("local_shell"), &tool("Bash")));
        assert!(matches(x, Some("apply_patch"), &tool("Edit")));
        assert!(matches(x, Some("apply_patch"), &tool("Write")));
        assert!(!matches(x, Some("apply_patch"), &tool("Read")));
        assert!(matches(x, Some("*"), &tool("Read")));
        assert!(!matches(x, Some("[bad"), &tool("Read")));
    }

    #[test]
    fn if_rules() {
        let bash = json!({ "command": "git status" });
        assert!(if_rule_matches("Bash(git *)", "Bash", &bash));
        assert!(if_rule_matches("Bash(git:*)", "Bash", &bash));
        assert!(!if_rule_matches("Bash(npm *)", "Bash", &bash));
        assert!(if_rule_matches("Bash", "Bash", &bash));
        assert!(!if_rule_matches("Edit(*.ts)", "Bash", &bash));
        let edit = json!({ "file_path": "src/a.ts" });
        assert!(if_rule_matches("Edit(*.ts)", "Edit", &edit));
        assert!(!if_rule_matches("Edit(*.rs)", "Edit", &edit));
        assert!(if_rule_matches(
            "Write(src/*)",
            "Write",
            &json!({ "path": "src/x" })
        ));
    }

    #[test]
    fn glob_cases() {
        assert!(glob("a*c", "abc"));
        assert!(glob("a*b*c", "a1b2c"));
        assert!(!glob("a*b*c", "a1c"));
        assert!(!glob("ab*ba", "aba"));
        assert!(glob("*", ""));
        assert!(glob("x", "x"));
    }
}
