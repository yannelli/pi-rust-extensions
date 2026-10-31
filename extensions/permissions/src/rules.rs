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
    CodexUser,
    CodexProject,
    PiGlobal,
    PiProject,
}

impl Source {
    pub const ALL: [Source; 8] = [
        Source::ClaudeManaged,
        Source::ClaudeUser,
        Source::ClaudeProject,
        Source::ClaudeLocal,
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
#[derive(Debug, Clone, PartialEq)]
pub struct PrefixRule {
    pub pattern: Vec<Vec<String>>,
}

impl PrefixRule {
    pub fn matches(&self, words: &[String]) -> bool {
        words.len() >= self.pattern.len()
            && self
                .pattern
                .iter()
                .zip(words)
                .all(|(alts, word)| alts.iter().any(|alt| alt == word))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Claude(ClaudeRule),
    Codex(PrefixRule),
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
    /// Precedence inside one target: deny, pi ask, pi allow, other ask, other allow.
    pub fn rank(&self) -> u8 {
        match (self.decision, self.source.is_pi()) {
            (Decision::Deny, _) => 4,
            (Decision::Ask, true) => 3,
            (Decision::Allow, true) => 2,
            (Decision::Ask, false) => 1,
            (Decision::Allow, false) => 0,
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
        let rule = PrefixRule {
            pattern: vec![vec!["git".into()], vec!["push".into(), "fetch".into()]],
        };
        let words = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert!(rule.matches(&words("git push origin")));
        assert!(rule.matches(&words("git fetch")));
        assert!(!rule.matches(&words("git status")));
        assert!(!rule.matches(&words("git")));
    }
}
