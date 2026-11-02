use std::path::Path;

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

/// Like `matches`; Claude StopFailure and FileChanged exact names allow only `[A-Za-z0-9_|]`.
pub fn matches_event(source: Source, event: &str, matcher: Option<&str>, target: &Target) -> bool {
    let narrow = !source.is_codex() && matches!(event, "StopFailure" | "FileChanged");
    match matcher.map(str::trim) {
        Some(m) if narrow && !m.is_empty() && m != "*" && !is_exact(m, "") => {
            regex_matches(m, &candidates(source, target))
        }
        _ => matches(source, matcher, target),
    }
}

fn candidates(source: Source, target: &Target) -> Vec<&str> {
    match target {
        Target::Any => Vec::new(),
        Target::Tool(name) if source.is_codex() => {
            let mut names = vec![name.as_str()];
            names.extend(codex_aliases(name));
            names
        }
        Target::Tool(name) | Target::Value(name) => vec![name.as_str()],
    }
}

fn regex_matches(matcher: &str, candidates: &[&str]) -> bool {
    match Regex::new(matcher) {
        Ok(regex) => candidates.iter().any(|c| regex.is_match(c)),
        Err(err) => {
            pi_ext_sdk::warn!("hooks: invalid matcher {matcher:?}: {err}");
            false
        }
    }
}

/// Exact-name matchers: Claude allows `[A-Za-z0-9_|]` plus `extra`; Codex allows `[A-Za-z0-9_|]`.
fn is_exact(matcher: &str, extra: &str) -> bool {
    matcher
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|' || extra.contains(c))
}

pub fn matches(source: Source, matcher: Option<&str>, target: &Target) -> bool {
    let matcher = matcher.unwrap_or("").trim();
    if matcher.is_empty() || matcher == "*" {
        return true;
    }
    if *target == Target::Any {
        return true;
    }
    let candidates = candidates(source, target);
    let extra = if source.is_codex() { "" } else { "- ," };
    if is_exact(matcher, extra) {
        return matcher
            .split(['|', ','])
            .map(str::trim)
            .any(|name| candidates.contains(&name));
    }
    regex_matches(matcher, &candidates)
}

/// Claude `if`: one permission rule such as `Bash(git *)` or `Edit(*.ts)`.
pub fn if_rule_matches(rule: &str, tool: &str, input: &Value, cwd: &Path) -> bool {
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
    let Some(value) = primary_input(tool, input) else {
        return false;
    };
    match tool {
        "Bash" | "PowerShell" => command_matches(&pattern, value),
        "Read" | "Edit" | "Write" | "LS" => {
            let relative = Path::new(value)
                .strip_prefix(cwd)
                .ok()
                .map(|p| p.display().to_string());
            glob(&pattern, value) || relative.is_some_and(|r| glob(&pattern, &r))
        }
        _ => glob(&pattern, value),
    }
}

/// Bash rules test each subcommand, with leading `VAR=value` removed.
fn command_matches(pattern: &str, command: &str) -> bool {
    let name_only = !pattern.trim_end_matches('*').trim_end().contains(' ');
    let dynamic = command.contains("$(") || command.contains('`') || command.contains('$');
    if !name_only && dynamic {
        return true;
    }
    match subcommands(command) {
        Some(commands) => commands.iter().any(|c| glob(pattern, c)),
        None => true,
    }
}

/// Splits on `&&`, `||`, `;`, `|`, `&`, and newlines outside quotes, and adds `$()` and
/// backtick bodies. `None` when the quoting does not close.
pub fn subcommands(command: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let (mut single, mut double) = (false, false);
    while let Some(c) = chars.next() {
        match c {
            '\\' if !single => {
                current.push(c);
                current.extend(chars.next());
            }
            '\'' if !double => {
                single = !single;
                current.push(c);
            }
            '"' if !single => {
                double = !double;
                current.push(c);
            }
            '$' if !single && chars.peek() == Some(&'(') => {
                chars.next();
                let body = take_until_close(&mut chars)?;
                out.extend(subcommands(&body)?);
                current.push_str("$()");
            }
            '`' if !single => {
                let body: String = chars.by_ref().take_while(|c| *c != '`').collect();
                out.extend(subcommands(&body)?);
                current.push_str("``");
            }
            ';' | '|' | '&' | '\n' if !single && !double => {
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    if single || double {
        return None;
    }
    out.push(current);
    Some(
        out.iter()
            .map(|c| strip_assignments(c.trim()))
            .filter(|c| !c.is_empty())
            .collect(),
    )
}

fn take_until_close(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    let mut depth = 1;
    let mut body = String::new();
    for c in chars.by_ref() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(body);
                }
            }
            _ => {}
        }
        body.push(c);
    }
    None
}

fn strip_assignments(command: &str) -> String {
    let mut rest = command;
    while let Some((word, tail)) = rest.split_once(char::is_whitespace) {
        let is_assignment = word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if !is_assignment {
            break;
        }
        rest = tail.trim_start();
    }
    rest.to_owned()
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
    fn codex_matchers() {
        let x = Source::Codex;
        assert!(!matches(x, Some("Bash"), &tool("BashOutput")), "exact name");
        assert!(matches(x, Some("Bash.*"), &tool("BashOutput")), "regex");
        assert!(!matches(x, Some("shell"), &tool("Bash")));
        assert!(matches(x, Some("apply_patch"), &tool("Edit")));
        assert!(matches(x, Some("Edit|Write"), &tool("Write")));
        assert!(!matches(x, Some("Edit"), &tool("NotebookEdit")));
        assert!(!matches(x, Some("apply_patch"), &tool("Read")));
        assert!(matches(x, Some("*"), &tool("Read")));
        assert!(!matches(x, Some("[bad"), &tool("Read")));
        let c = Source::Claude;
        let value = Target::Value("rate_limit".into());
        assert!(matches_event(
            c,
            "StopFailure",
            Some("rate_limit|unknown"),
            &value
        ));
        assert!(!matches_event(
            c,
            "StopFailure",
            Some("rate_limit, unknown"),
            &value
        ));
    }

    #[test]
    fn if_rules() {
        let cwd = Path::new("/r");
        let bash = json!({ "command": "git status" });
        assert!(if_rule_matches("Bash(git *)", "Bash", &bash, cwd));
        assert!(if_rule_matches("Bash(git:*)", "Bash", &bash, cwd));
        assert!(!if_rule_matches("Bash(npm *)", "Bash", &bash, cwd));
        assert!(if_rule_matches("Bash", "Bash", &bash, cwd));
        assert!(!if_rule_matches("Edit(*.ts)", "Bash", &bash, cwd));
        let edit = json!({ "file_path": "/r/src/a.ts" });
        assert!(if_rule_matches("Edit(*.ts)", "Edit", &edit, cwd));
        assert!(!if_rule_matches("Edit(*.rs)", "Edit", &edit, cwd));
        assert!(if_rule_matches("Edit(src/**)", "Edit", &edit, cwd));
        assert!(if_rule_matches("Edit(/r/src/*)", "Edit", &edit, cwd));
    }

    #[test]
    fn bash_if_rules_check_subcommands() {
        let cwd = Path::new("/r");
        let run = |rule: &str, command: &str| {
            if_rule_matches(rule, "Bash", &json!({ "command": command }), cwd)
        };
        assert!(run("Bash(git *)", "FOO=bar git push"));
        assert!(run("Bash(git *)", "npm test && git push"));
        assert!(run("Bash(rm *)", "echo $(rm -rf /)"));
        assert!(run("Bash(rm *)", "echo `rm -rf x`"));
        assert!(!run("Bash(rm *)", "echo $(date)"));
        assert!(run("Bash(git push *)", "echo $(date)"));
        assert!(!run("Bash(rm *)", "echo 'a && rm x'"));
        assert!(run("Bash(rm *)", "echo 'unclosed"));
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
