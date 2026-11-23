//! iTerm2 escape sequences and the agent states.

use std::path::Path;

/// The footer `runState` of pi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramState {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

impl ProgramState {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "idle" => Some(Self::Idle),
            "working" => Some(Self::Working),
            "blocked" => Some(Self::Blocked),
            "done" => Some(Self::Done),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Environment lookups, so tests do not touch the process environment.
pub trait EnvLookup {
    fn var(&self, name: &str) -> Option<String>;

    fn non_empty(&self, name: &str) -> Option<String> {
        self.var(name).filter(|value| !value.is_empty())
    }
}

pub struct ProcessEnv;

impl EnvLookup for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

impl EnvLookup for std::collections::HashMap<String, String> {
    fn var(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

/// iTerm2 sets `TERM_PROGRAM`; `LC_TERMINAL` also reaches tmux and SSH sessions.
pub fn is_iterm2(env: &dyn EnvLookup) -> bool {
    env.var("TERM_PROGRAM")
        .is_some_and(|program| program == "iTerm.app")
        || env
            .var("LC_TERMINAL")
            .is_some_and(|terminal| terminal == "iTerm2")
        || env.non_empty("ITERM_SESSION_ID").is_some()
}

/// Wraps an escape sequence for tmux, which passes it on with `set -g allow-passthrough on`.
pub fn passthrough(sequence: &str, tmux: bool) -> String {
    if tmux {
        format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b"))
    } else {
        sequence.to_string()
    }
}

/// `OSC 6` tab color; `None` restores the profile color.
pub fn tab_color(rgb: Option<[u8; 3]>) -> String {
    match rgb {
        Some([r, g, b]) => format!(
            "\x1b]6;1;bg;red;brightness;{r}\x07\x1b]6;1;bg;green;brightness;{g}\x07\x1b]6;1;bg;blue;brightness;{b}\x07"
        ),
        None => "\x1b]6;1;bg;*;default\x07".to_string(),
    }
}

/// `OSC 1337 SetUserVar`; iTerm2 shows it as `\(user.<name>)`.
pub fn user_var(name: &str, value: &str) -> String {
    format!("\x1b]1337;SetUserVar={name}={}\x07", base64(value))
}

/// `OSC 1337 SetBadgeFormat`; an empty text removes the badge.
pub fn badge(text: &str) -> String {
    format!("\x1b]1337;SetBadgeFormat={}\x07", base64(text))
}

pub fn request_attention() -> &'static str {
    "\x1b]1337;RequestAttention=yes\x07"
}

pub fn current_dir(path: &Path) -> String {
    let path: String = path
        .display()
        .to_string()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    format!("\x1b]1337;CurrentDir={path}\x07")
}

pub fn state_name(state: ProgramState) -> &'static str {
    match state {
        ProgramState::Idle => "idle",
        ProgramState::Working => "working",
        ProgramState::Blocked => "blocked",
        ProgramState::Done => "done",
        ProgramState::Error => "error",
    }
}

/// Standard base64 with padding.
pub fn base64(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn detects_iterm2_directly_and_through_tmux_or_ssh() {
        let env = |vars: &[(&str, &str)]| -> HashMap<String, String> {
            vars.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert!(is_iterm2(&env(&[("TERM_PROGRAM", "iTerm.app")])));
        assert!(is_iterm2(&env(&[
            ("TERM_PROGRAM", "tmux"),
            ("LC_TERMINAL", "iTerm2")
        ])));
        assert!(is_iterm2(&env(&[("ITERM_SESSION_ID", "w0t0p0:1")])));
        assert!(!is_iterm2(&env(&[("TERM_PROGRAM", "ghostty")])));
        assert!(!is_iterm2(&env(&[])));
    }

    #[test]
    fn builds_iterm2_sequences() {
        assert_eq!(
            tab_color(Some([1, 2, 3])),
            "\x1b]6;1;bg;red;brightness;1\x07\x1b]6;1;bg;green;brightness;2\x07\x1b]6;1;bg;blue;brightness;3\x07"
        );
        assert_eq!(tab_color(None), "\x1b]6;1;bg;*;default\x07");
        assert_eq!(
            user_var("pi_state", "working"),
            "\x1b]1337;SetUserVar=pi_state=d29ya2luZw==\x07"
        );
        assert_eq!(badge("π x"), "\x1b]1337;SetBadgeFormat=z4AgeA==\x07");
        assert_eq!(badge(""), "\x1b]1337;SetBadgeFormat=\x07");
        assert_eq!(
            current_dir(Path::new("/w/a\nb")),
            "\x1b]1337;CurrentDir=/w/ab\x07"
        );
        assert_eq!(
            passthrough("\x1b]9;x\x07", true),
            "\x1bPtmux;\x1b\x1b]9;x\x07\x1b\\"
        );
        assert_eq!(passthrough("\x1b]9;x\x07", false), "\x1b]9;x\x07");
        assert_eq!(state_name(ProgramState::Blocked), "blocked");
        assert_eq!(ProgramState::parse("done"), Some(ProgramState::Done));
        assert_eq!(ProgramState::parse("clear"), None);
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        for (text, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
        ] {
            assert_eq!(base64(text), encoded);
        }
    }
}
