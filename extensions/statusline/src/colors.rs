//! ANSI colors. pi theme tokens map to basic terminal colors, which follow the terminal
//! palette; theme colors are truecolor, or the nearest of 256 colors without truecolor.

use std::sync::OnceLock;

pub const RESET: &str = "\x1b[0m";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Truecolor,
    Ansi256,
}

static MODE: OnceLock<ColorMode> = OnceLock::new();

/// Sets the mode from `COLORTERM` and `TERM_PROGRAM`, once per process.
pub fn init_mode(colorterm: Option<&str>, term_program: Option<&str>) {
    let truecolor = matches!(colorterm, Some("truecolor" | "24bit"))
        || term_program.is_some_and(|program| program != "Apple_Terminal");
    let _ = MODE.set(if truecolor {
        ColorMode::Truecolor
    } else {
        ColorMode::Ansi256
    });
}

pub fn mode() -> ColorMode {
    *MODE.get().unwrap_or(&ColorMode::Truecolor)
}

/// A pi theme token that a `colors` entry or the `terminal` theme names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeColor {
    Accent,
    Text,
    Muted,
    Dim,
    Success,
    Warning,
    Error,
    Border,
}

impl ThemeColor {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "accent" => Some(Self::Accent),
            "text" => Some(Self::Text),
            "muted" => Some(Self::Muted),
            "dim" => Some(Self::Dim),
            "success" => Some(Self::Success),
            "warning" => Some(Self::Warning),
            "error" => Some(Self::Error),
            "border" | "borderAccent" | "borderMuted" => Some(Self::Border),
            _ => None,
        }
    }

    fn sgr(self) -> &'static str {
        match self {
            Self::Accent => "36",
            Self::Text => "39",
            Self::Muted => "37",
            Self::Dim | Self::Border => "90",
            Self::Success => "32",
            Self::Warning => "33",
            Self::Error => "31",
        }
    }

    /// An sRGB stand-in for blocks colored by a token.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Self::Accent => [0x56, 0xB6, 0xC2],
            Self::Text => [0xD4, 0xD4, 0xD4],
            Self::Muted => [0xA3, 0xA3, 0xA3],
            Self::Dim | Self::Border => [0x73, 0x73, 0x73],
            Self::Success => [0x98, 0xC3, 0x79],
            Self::Warning => [0xE5, 0xC0, 0x7B],
            Self::Error => [0xE0, 0x6C, 0x75],
        }
    }

    pub fn paint(self, text: &str) -> String {
        format!("\x1b[{}m{text}\x1b[39m", self.sgr())
    }
}

fn ansi256([r, g, b]: [u8; 3]) -> u8 {
    let cube = |c: u8| {
        if c < 48 {
            0
        } else if c < 115 {
            1
        } else {
            (c - 35) / 40
        }
    };
    16 + 36 * cube(r) + 6 * cube(g) + cube(b)
}

fn sgr(base: u8, rgb: [u8; 3]) -> String {
    let [r, g, b] = rgb;
    match mode() {
        ColorMode::Truecolor => format!("\x1b[{base};2;{r};{g};{b}m"),
        ColorMode::Ansi256 => format!("\x1b[{base};5;{}m", ansi256(rgb)),
    }
}

pub fn foreground(rgb: [u8; 3]) -> String {
    sgr(38, rgb)
}

pub fn background(rgb: [u8; 3]) -> String {
    sgr(48, rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_cube_colors() {
        assert_eq!(ThemeColor::from_name("accent"), Some(ThemeColor::Accent));
        assert_eq!(ThemeColor::from_name("#fff"), None);
        assert_eq!(ThemeColor::Error.paint("x"), "\x1b[31mx\x1b[39m");
        assert_eq!(ansi256([0, 0, 0]), 16);
        assert_eq!(ansi256([255, 255, 255]), 231);
        assert_eq!(foreground([1, 2, 3]), "\x1b[38;2;1;2;3m");
    }
}
