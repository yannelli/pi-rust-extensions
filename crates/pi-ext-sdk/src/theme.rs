//! pi's TUI theme (`ui.getTheme`, `theme.changed`): the SGR sequence pi draws each token with,
//! so extension lines match the rest of the TUI.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const FG_CLOSE: &str = "\x1b[39m";
const BG_CLOSE: &str = "\x1b[49m";
const DIM_OPEN: &str = "\x1b[2m";
const DIM_CLOSE: &str = "\x1b[22m";

/// The theme as pi sends it. Tokens are those of the theme JSON, such as `accent`, `muted`,
/// `toolTitle`, and the backgrounds `selectedBg` and `toolSuccessBg`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Theme {
    pub name: Option<String>,
    pub source_path: Option<String>,
    /// `dark` or `light`.
    pub appearance: String,
    /// `truecolor` or `256color`.
    pub mode: String,
    pub fg: BTreeMap<String, String>,
    pub bg: BTreeMap<String, String>,
    /// Foreground tokens that pi also draws dim.
    pub dim: Vec<String>,
}

impl Theme {
    pub fn is_light(&self) -> bool {
        self.appearance == "light"
    }

    /// `text` in the foreground of `token`; unchanged for an unknown token.
    pub fn fg(&self, token: &str, text: &str) -> String {
        let Some(open) = self.fg.get(token) else {
            return text.to_owned();
        };
        if self.dim.iter().any(|name| name == token) {
            format!("{open}{DIM_OPEN}{text}{DIM_CLOSE}{FG_CLOSE}")
        } else {
            format!("{open}{text}{FG_CLOSE}")
        }
    }

    /// `text` on the background of `token`; unchanged for an unknown token.
    pub fn bg(&self, token: &str, text: &str) -> String {
        match self.bg.get(token) {
            Some(open) => format!("{open}{text}{BG_CLOSE}"),
            None => text.to_owned(),
        }
    }

    /// The open sequence of a foreground token, with dim when pi draws it dim.
    pub fn fg_open(&self, token: &str) -> Option<String> {
        let open = self.fg.get(token)?;
        let dim = self.dim.iter().any(|name| name == token);
        Some(if dim {
            format!("{open}{DIM_OPEN}")
        } else {
            open.clone()
        })
    }

    /// The sRGB color of a foreground token; `None` for the terminal default or an unknown token.
    pub fn rgb(&self, token: &str) -> Option<[u8; 3]> {
        self.fg.get(token).and_then(|open| sgr_rgb(open))
    }

    /// The sRGB color of a background token.
    pub fn bg_rgb(&self, token: &str) -> Option<[u8; 3]> {
        self.bg.get(token).and_then(|open| sgr_rgb(open))
    }
}

/// The color of an SGR sequence `ESC[38;2;r;g;bm`, `ESC[48;5;nm`, and the like.
pub fn sgr_rgb(sequence: &str) -> Option<[u8; 3]> {
    let body = sequence.strip_prefix("\x1b[")?.strip_suffix('m')?;
    let parts: Vec<u16> = body
        .split(';')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let byte = |value: u16| u8::try_from(value).ok();
    match parts.as_slice() {
        [38 | 48, 2, r, g, b, ..] => Some([byte(*r)?, byte(*g)?, byte(*b)?]),
        [38 | 48, 5, index, ..] => Some(ansi256_rgb(byte(*index)?)),
        _ => None,
    }
}

/// The xterm color of a 256-color index; the 16 basic colors use the xterm defaults.
pub fn ansi256_rgb(index: u8) -> [u8; 3] {
    const BASIC: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 0, 0],
        [0, 205, 0],
        [205, 205, 0],
        [0, 0, 238],
        [205, 0, 205],
        [0, 205, 205],
        [229, 229, 229],
        [127, 127, 127],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [92, 92, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    match index {
        0..=15 => BASIC[usize::from(index)],
        16..=231 => {
            let cube = index - 16;
            let level = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
            [level(cube / 36), level(cube / 6 % 6), level(cube % 6)]
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            [gray, gray, gray]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn theme() -> Theme {
        serde_json::from_value(json!({
            "name": "dark", "appearance": "dark", "mode": "truecolor",
            "fg": { "accent": "\u{1b}[38;2;138;180;248m", "dim": "\u{1b}[38;5;245m", "text": "\u{1b}[39m" },
            "bg": { "selectedBg": "\u{1b}[48;2;1;2;3m" },
            "dim": ["dim"],
            "unknown": true
        }))
        .unwrap()
    }

    #[test]
    fn paints_tokens_and_reads_their_colors() {
        let theme = theme();
        assert_eq!(theme.fg("accent", "a"), "\x1b[38;2;138;180;248ma\x1b[39m");
        assert_eq!(
            theme.fg("dim", "d"),
            "\x1b[38;5;245m\x1b[2md\x1b[22m\x1b[39m"
        );
        assert_eq!(theme.fg("nope", "x"), "x");
        assert_eq!(theme.bg("selectedBg", "b"), "\x1b[48;2;1;2;3mb\x1b[49m");
        assert_eq!(theme.rgb("accent"), Some([138, 180, 248]));
        assert_eq!(theme.rgb("dim"), Some([138, 138, 138]));
        assert_eq!(theme.rgb("text"), None);
        assert_eq!(theme.bg_rgb("selectedBg"), Some([1, 2, 3]));
        assert!(!theme.is_light());
    }

    #[test]
    fn maps_256_color_indexes() {
        assert_eq!(ansi256_rgb(16), [0, 0, 0]);
        assert_eq!(ansi256_rgb(231), [255, 255, 255]);
        assert_eq!(ansi256_rgb(196), [255, 0, 0]);
        assert_eq!(ansi256_rgb(255), [238, 238, 238]);
        assert_eq!(sgr_rgb("\x1b[38;5;9m"), Some([255, 0, 0]));
        assert_eq!(sgr_rgb("\x1b[1m"), None);
    }
}
