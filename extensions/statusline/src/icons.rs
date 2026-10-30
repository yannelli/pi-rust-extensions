//! Icon sets for status line segments.

use crate::segments::Segment;

/// `unicode` (the default), `nerd` (needs a Nerd Font), `emoji`, or `none`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icons {
    Unicode,
    Nerd,
    Emoji,
    None,
}

impl Icons {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "unicode" => Some(Icons::Unicode),
            "nerd" => Some(Icons::Nerd),
            "emoji" => Some(Icons::Emoji),
            "none" => Some(Icons::None),
            _ => None,
        }
    }

    /// The icon before a segment, if the set has one.
    pub fn icon(self, segment: Segment) -> Option<&'static str> {
        use Segment::*;
        match (self, segment) {
            (Icons::None, _) => None,
            (Icons::Unicode, Branch) => Some("⎇"),
            (Icons::Unicode, Time) => Some("◷"),
            (Icons::Unicode, _) => None,
            (Icons::Nerd, Cwd | Dir | Project) => Some("\u{f07b}"),
            (Icons::Nerd, Branch) => Some("\u{e0a0}"),
            (Icons::Nerd, Model | ModelWithReasoning) => Some("\u{f06a9}"),
            (Icons::Nerd, Thinking) => Some("\u{f0335}"),
            (Icons::Nerd, Cost) => Some("\u{f155}"),
            (
                Icons::Nerd,
                Context | ContextUsed | ContextRemaining | ContextWindow | ContextBar
                | ContextTokens | ContextHeadroom,
            ) => Some("\u{f080}"),
            (Icons::Nerd, Tokens | TokensIn | TokensOut | TokensUsed) => Some("\u{f0e7}"),
            (Icons::Nerd, Usage | UsageReset) => Some("\u{f0e4}"),
            (Icons::Nerd, Time) => Some("\u{f017}"),
            (Icons::Nerd, Session | SessionId) => Some("\u{f075}"),
            (Icons::Nerd, User) => Some("\u{f007}"),
            (Icons::Nerd, Host) => Some("\u{f108}"),
            (Icons::Emoji, Cwd | Dir | Project) => Some("📁"),
            (Icons::Emoji, Branch) => Some("🌿"),
            (Icons::Emoji, Model | ModelWithReasoning) => Some("🤖"),
            (Icons::Emoji, Thinking) => Some("🧠"),
            (Icons::Emoji, Cost) => Some("💰"),
            (
                Icons::Emoji,
                Context | ContextUsed | ContextRemaining | ContextWindow | ContextBar
                | ContextTokens | ContextHeadroom,
            ) => Some("📊"),
            (Icons::Emoji, Tokens | TokensIn | TokensOut | TokensUsed) => Some("⚡"),
            (Icons::Emoji, Usage | UsageReset) => Some("⏳"),
            (Icons::Emoji, Time) => Some("⏱"),
            (Icons::Emoji, Session | SessionId) => Some("💬"),
            _ => None,
        }
    }
}
