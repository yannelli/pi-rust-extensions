//! ANSI colors. pi theme tokens use the colors of pi's TUI theme (`ui.getTheme`); a host
//! without it maps them to basic terminal colors. Theme colors are truecolor, or the nearest of
//! 256 colors without truecolor.

use std::sync::{OnceLock, PoisonError, RwLock};

use pi_ext_sdk::{Host, Theme};

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
    if let Some(mode) = host_colors(|colors| colors.mode) {
        return mode;
    }
    *MODE.get().unwrap_or(&ColorMode::Truecolor)
}

/// The open sequence and sRGB color of each token in pi's theme, and pi's color mode.
struct HostColors {
    open: [Option<String>; TOKENS.len()],
    rgb: [Option<[u8; 3]>; TOKENS.len()],
    mode: ColorMode,
}

static HOST: RwLock<Option<HostColors>> = RwLock::new(None);

fn host_colors<T>(read: impl FnOnce(&HostColors) -> T) -> Option<T> {
    HOST.read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(read)
}

/// Paints tokens with the colors of `theme` from now on.
pub fn set_theme(theme: &Theme) {
    let colors = HostColors {
        open: TOKENS.map(|token| theme.fg_open(token.name())),
        rgb: TOKENS.map(|token| theme.rgb(token.name())),
        mode: if theme.mode == "256color" {
            ColorMode::Ansi256
        } else {
            ColorMode::Truecolor
        },
    };
    *HOST.write().unwrap_or_else(PoisonError::into_inner) = Some(colors);
}

/// Reads pi's theme when the host has `ui.getTheme`; older hosts keep the basic colors.
pub async fn load_theme(host: &Host) {
    if !host
        .init()
        .is_some_and(|init| init.has_feature("ui.getTheme"))
    {
        return;
    }
    if let Ok(theme) = host.theme().await {
        set_theme(&theme);
    }
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

const TOKENS: [ThemeColor; 8] = [
    ThemeColor::Accent,
    ThemeColor::Text,
    ThemeColor::Muted,
    ThemeColor::Dim,
    ThemeColor::Success,
    ThemeColor::Warning,
    ThemeColor::Error,
    ThemeColor::Border,
];

impl ThemeColor {
    /// The token name in pi's theme.
    pub fn name(self) -> &'static str {
        match self {
            Self::Accent => "accent",
            Self::Text => "text",
            Self::Muted => "muted",
            Self::Dim => "dim",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Border => "border",
        }
    }

    fn index(self) -> usize {
        TOKENS.iter().position(|token| *token == self).unwrap_or(0)
    }

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

    /// The sRGB color for blocks colored by a token: pi's theme color, else a stand-in.
    pub fn rgb(self) -> [u8; 3] {
        if let Some(Some(rgb)) = host_colors(|colors| colors.rgb[self.index()]) {
            return rgb;
        }
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

    /// The escape that starts this color, as [`ThemeColor::paint`] writes it.
    pub fn open(self) -> String {
        match host_colors(|colors| colors.open[self.index()].clone()).flatten() {
            Some(open) => open,
            None => format!("\x1b[{}m", self.sgr()),
        }
    }

    pub fn paint(self, text: &str) -> String {
        match host_colors(|colors| colors.open[self.index()].clone()).flatten() {
            Some(open) if open.ends_with("\x1b[2m") => format!("{open}{text}\x1b[22m\x1b[39m"),
            Some(open) => format!("{open}{text}\x1b[39m"),
            None => format!("\x1b[{}m{text}\x1b[39m", self.sgr()),
        }
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

/// A mild variation of `base` for part `index` of `count`: the hue turns 12 degrees and the
/// lightness moves 3% per part from the middle part, which keeps `base`.
pub fn shade(base: [u8; 3], index: usize, count: usize) -> [u8; 3] {
    let offset = index as f64 - count.saturating_sub(1) as f64 / 2.0;
    let [r, g, b] = base.map(|c| f64::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    let saturation = if delta == 0.0 {
        0.0
    } else {
        delta / (1.0 - (2.0 * lightness - 1.0).abs())
    };
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let step = offset * 0.03;
    // Near white or black, the lightness moves the other way so the parts still differ.
    let moved = if (0.05..=0.95).contains(&(lightness + step)) {
        lightness + step
    } else {
        lightness - step
    };
    hsl_to_rgb(
        (hue + offset * 12.0).rem_euclid(360.0),
        saturation,
        moved.clamp(0.0, 1.0),
    )
}

fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> [u8; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hue as u32 / 60 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    [r, g, b].map(|c| ((c + m) * 255.0).round().clamp(0.0, 255.0) as u8)
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
    fn shades_keep_the_middle_and_vary_the_rest() {
        let accent = [0x8a, 0xbe, 0xb7];
        assert_eq!(shade(accent, 4, 9), accent);
        let shades: Vec<_> = (0..9).map(|index| shade(accent, index, 9)).collect();
        for pair in shades.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
        // White has no hue, so only the lightness moves, downward.
        let white = [255, 255, 255];
        assert_eq!(shade(white, 4, 9), white);
        assert!(shade(white, 8, 9)[0] < 255 && shade(white, 0, 9)[0] < 255);
    }

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
