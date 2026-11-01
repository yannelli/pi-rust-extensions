use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

impl Decision {
    pub const ALL: [Decision; 3] = [Decision::Deny, Decision::Ask, Decision::Allow];

    pub fn name(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Ask => "ask",
            Decision::Deny => "deny",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.name() == text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    ClaudeManaged,
    ClaudeUser,
    ClaudeProject,
    ClaudeLocal,
    CodexManaged,
    CodexSystem,
    CodexUser,
    CodexProject,
    PiGlobal,
    PiProject,
}

impl Source {
    pub const ALL: [Source; 10] = [
        Source::ClaudeManaged,
        Source::ClaudeUser,
        Source::ClaudeProject,
        Source::ClaudeLocal,
        Source::CodexManaged,
        Source::CodexSystem,
        Source::CodexUser,
        Source::CodexProject,
        Source::PiGlobal,
        Source::PiProject,
    ];

    /// The key in the `sources` option; both pi files share `pi`.
    pub fn key(self) -> &'static str {
        match self {
            Source::ClaudeManaged => "claude-managed",
            Source::ClaudeUser => "claude-user",
            Source::ClaudeProject => "claude-project",
            Source::ClaudeLocal => "claude-local",
            Source::CodexManaged => "codex-managed",
            Source::CodexSystem => "codex-system",
            Source::CodexUser => "codex-user",
            Source::CodexProject => "codex-project",
            Source::PiGlobal | Source::PiProject => "pi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::PiGlobal => "pi-global",
            Source::PiProject => "pi-project",
            other => other.key(),
        }
    }

    pub fn is_project(self) -> bool {
        matches!(
            self,
            Source::ClaudeProject | Source::ClaudeLocal | Source::CodexProject | Source::PiProject
        )
    }

    pub fn is_pi(self) -> bool {
        matches!(self, Source::PiGlobal | Source::PiProject)
    }

    pub fn toggle_keys() -> Vec<&'static str> {
        let mut keys: Vec<&str> = Self::ALL.iter().map(|s| s.key()).collect();
        keys.dedup();
        keys
    }
}

/// A Claude-syntax rule: `Tool` or `Tool(specifier)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeRule {
    pub tool: String,
    pub spec: Option<String>,
    /// Where a `/path` pattern anchors.
    pub anchor: PathBuf,
}

/// One Codex `prefix_rule` with a single first token; each later position lists alternatives.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PrefixRule {
    pub pattern: Vec<Vec<String>>,
    /// The `host_executable` paths of the program; `None` lets any absolute path fall back.
    pub hosts: Option<Vec<String>>,
}

impl PrefixRule {
    /// An exact first token matches first; an absolute program path falls back to its
    /// basename when `host_executable` allows that path.
    pub fn matches(&self, words: &[String]) -> bool {
        let (Some(first), Some(program)) = (self.pattern.first(), words.first()) else {
            return false;
        };
        let exact = first.contains(program);
        let base = program.rsplit(['/', '\\']).next().unwrap_or_default();
        let fallback = std::path::Path::new(program).is_absolute()
            && first.iter().any(|alt| alt == base)
            && self
                .hosts
                .as_ref()
                .is_none_or(|hosts| hosts.contains(program));
        words.len() >= self.pattern.len()
            && (exact || fallback)
            && self.pattern[1..]
                .iter()
                .zip(&words[1..])
                .all(|(alts, word)| alts.iter().any(|alt| alt == word))
    }
}

/// A Codex `network_rule`: a decision for one host, applied to pi's fetch tools.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkRule {
    pub host: String,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Claude(ClaudeRule),
    Codex(PrefixRule),
    Network(NetworkRule),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub source: Source,
    pub decision: Decision,
    /// The rule as written in its file.
    pub text: String,
    pub file: PathBuf,
    pub justification: Option<String>,
    pub kind: Kind,
}

impl Rule {
    /// Precedence inside one target: deny, then ask, then allow, the same for every source.
    pub fn rank(&self) -> u8 {
        match self.decision {
            Decision::Deny => 2,
            Decision::Ask => 1,
            Decision::Allow => 0,
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{} ({}, {})",
            self.text,
            self.source.label(),
            self.file.display()
        )
    }
}

/// Splits `Tool(specifier)`; `Tool(*)` and `Tool()` have no specifier.
pub fn split_claude(text: &str) -> Option<(String, Option<String>)> {
    let text = text.trim();
    let (tool, spec) = match text.find('(') {
        Some(open) if text.ends_with(')') => (&text[..open], Some(&text[open + 1..text.len() - 1])),
        Some(_) => return None,
        None => (text, None),
    };
    let tool = tool.trim();
    if tool.is_empty() || tool.contains(char::is_whitespace) {
        return None;
    }
    let spec = spec
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "*")
        .map(str::to_owned);
    Some((tool.to_owned(), spec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_claude_rules() {
        assert_eq!(split_claude("Bash"), Some(("Bash".into(), None)));
        assert_eq!(split_claude("Bash(*)"), Some(("Bash".into(), None)));
        assert_eq!(
            split_claude("Bash(npm run test:*)"),
            Some(("Bash".into(), Some("npm run test:*".into())))
        );
        assert_eq!(
            split_claude("Edit(./Finance (2024)/**)"),
            Some(("Edit".into(), Some("./Finance (2024)/**".into())))
        );
        assert_eq!(split_claude("Bash(oops"), None);
        assert_eq!(split_claude(""), None);
    }

    #[test]
    fn prefix_rules_match_alternatives() {
        let mut rule = PrefixRule {
            pattern: vec![vec!["git".into()], vec!["push".into(), "fetch".into()]],
            hosts: None,
        };
        let words = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert!(rule.matches(&words("git push origin")));
        assert!(rule.matches(&words("git fetch")));
        assert!(!rule.matches(&words("git status")));
        assert!(!rule.matches(&words("git")));
        assert!(rule.matches(&words("/usr/bin/git push")));
        rule.hosts = Some(vec!["/opt/homebrew/bin/git".into()]);
        assert!(!rule.matches(&words("/usr/bin/git push")));
        assert!(rule.matches(&words("/opt/homebrew/bin/git push")));
        assert!(rule.matches(&words("git push")));
    }
}
