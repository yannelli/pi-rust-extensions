//! iTerm2 escape sequences and the colors for each agent state.

use std::path::Path;

// iTerm2 picks tab-title ink at a brightness cutoff, not WCAG contrast. Keep backgrounds
// light enough for black titles, including its inactive-tab blend toward middle gray.
pub const WORKING: [u8; 3] = [132, 184, 255];
pub const BLOCKED: [u8; 3] = [242, 202, 114];
pub const DONE: [u8; 3] = [151, 211, 163];
pub const ERROR: [u8; 3] = [245, 160, 164];

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

/// The tab color for a state; idle uses the profile color.
pub fn state_color(state: ProgramState) -> Option<[u8; 3]> {
    match state {
        ProgramState::Working => Some(WORKING),
        ProgramState::Blocked => Some(BLOCKED),
        ProgramState::Done => Some(DONE),
        ProgramState::Error => Some(ERROR),
        ProgramState::Idle => None,
    }
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

    const COLORED: [ProgramState; 4] = [
        ProgramState::Working,
        ProgramState::Blocked,
        ProgramState::Done,
        ProgramState::Error,
    ];

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
    fn state_colors_keep_black_titles_readable_when_tabs_are_dimmed() {
        let linear = |value: f64| {
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        for state in COLORED {
            let rgb = state_color(state).expect("colored state");
            // Selected/unselected tabs in active/inactive windows use these blends in iTerm2.
            for alpha in [1.0, 0.6, 0.4, 0.3] {
                let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0 * alpha + 0.5 * (1.0 - alpha));
                assert!(
                    0.299 * r + 0.587 * g + 0.114 * b > 0.5,
                    "{state:?}, alpha {alpha}"
                );
                let luminance = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
                assert!((luminance + 0.05) / 0.05 >= 4.5, "{state:?}, alpha {alpha}");
            }
        }
        assert_eq!(state_color(ProgramState::Idle), None);
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
        assert_eq!(state_color(ProgramState::Working), Some(WORKING));
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
