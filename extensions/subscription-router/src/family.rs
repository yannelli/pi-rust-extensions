//! Subscription families and their numbered provider slots (`openai-codex`, `openai-codex-2`, ...).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Codex,
    Anthropic,
}

impl Family {
    pub const ALL: [Family; 2] = [Family::Codex, Family::Anthropic];

    /// The built-in pi provider, which is slot 1.
    pub fn base(self) -> &'static str {
        match self {
            Self::Codex => "openai-codex",
            Self::Anthropic => "anthropic",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "OpenAI Codex",
            Self::Anthropic => "Anthropic",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Anthropic => "Claude",
        }
    }

    pub fn provider(self, slot: u32) -> String {
        if slot <= 1 {
            self.base().to_owned()
        } else {
            format!("{}-{slot}", self.base())
        }
    }

    pub fn slot_name(self, slot: u32) -> String {
        format!("{} #{slot}", self.label())
    }
}

/// The family and slot number of a provider id; `None` for other providers.
pub fn parse_provider(provider: &str) -> Option<(Family, u32)> {
    Family::ALL.into_iter().find_map(|family| {
        let base = family.base();
        if provider == base {
            return Some((family, 1));
        }
        let rest = provider.strip_prefix(base)?.strip_prefix('-')?;
        let slot: u32 = rest.parse().ok().filter(|n| *n >= 2)?;
        (rest == slot.to_string()).then_some((family, slot))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slot_providers() {
        assert_eq!(parse_provider("openai-codex"), Some((Family::Codex, 1)));
        assert_eq!(parse_provider("openai-codex-3"), Some((Family::Codex, 3)));
        assert_eq!(parse_provider("anthropic-2"), Some((Family::Anthropic, 2)));
        assert_eq!(parse_provider("anthropic-1"), None);
        assert_eq!(parse_provider("anthropic-02"), None);
        assert_eq!(parse_provider("openai"), None);
        assert_eq!(Family::Codex.provider(1), "openai-codex");
        assert_eq!(Family::Anthropic.provider(4), "anthropic-4");
    }
}
