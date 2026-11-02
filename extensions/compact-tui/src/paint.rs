//! Colors from pi's TUI theme, basic ANSI colors for hosts without `ui.getTheme`, and the
//! frames of the working indicator.

use pi_ext_sdk::Theme;

/// Spinner frames, the same as pi's own.
pub const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Frames of one shimmer pass: three spinner turns.
const SHIMMER_FRAMES: usize = 30;

/// How lines are painted: pi's theme when the host sent it.
#[derive(Clone, Debug, Default)]
pub struct Paint {
    theme: Option<Theme>,
}

impl Paint {
    pub fn new(theme: Option<Theme>) -> Self {
        Self { theme }
    }

    fn known(&self, token: &str) -> Option<&Theme> {
        self.theme
            .as_ref()
            .filter(|theme| theme.fg.contains_key(token))
    }

    pub fn fg(&self, token: &str, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        match self.known(token) {
            Some(theme) => theme.fg(token, text),
            None => format!("\x1b[{}m{text}\x1b[39m", basic(token)),
        }
    }

    /// `text` on a theme background, or unchanged when the theme has no such token.
    pub fn bg(&self, token: &str, text: &str) -> String {
        match &self.theme {
            Some(theme) if theme.bg.contains_key(token) => theme.bg(token, text),
            _ => text.to_owned(),
        }
    }

    pub fn has_bg(&self, token: &str) -> bool {
        self.theme
            .as_ref()
            .is_some_and(|theme| theme.bg.contains_key(token))
    }

    pub fn bold(text: &str) -> String {
        format!("\x1b[1m{text}\x1b[22m")
    }

    pub fn rgb(&self, token: &str) -> Option<[u8; 3]> {
        self.theme.as_ref()?.rgb(token)
    }

    fn truecolor(&self) -> bool {
        self.theme
            .as_ref()
            .is_some_and(|theme| theme.mode == "truecolor")
    }

    fn is_light(&self) -> bool {
        self.theme.as_ref().is_some_and(Theme::is_light)
    }
}

/// The basic SGR foreground of a token without a theme.
fn basic(token: &str) -> &'static str {
    match token {
        "accent" | "toolTitle" => "36",
        "success" | "toolDiffAdded" => "32",
        "error" | "toolDiffRemoved" => "31",
        "warning" => "33",
        "text" => "39",
        _ => "90",
    }
}

fn mix(from: [u8; 3], to: [u8; 3], amount: f32) -> [u8; 3] {
    let channel =
        |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    [
        channel(from[0], to[0]),
        channel(from[1], to[1]),
        channel(from[2], to[2]),
    ]
}

fn rgb_fg([r, g, b]: [u8; 3]) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

/// Frames of the working indicator: the spinner and `label`. With motion and a truecolor theme,
/// a light band runs across the label; with a 256-color theme the label stays in the accent
/// color; without motion one still frame shows.
pub fn working_frames(paint: &Paint, label: &str, motion: bool) -> Vec<String> {
    if !motion {
        return vec![format!(
            "{} {}",
            paint.fg("accent", "●"),
            paint.fg("accent", label)
        )];
    }
    let Some(base) = paint.rgb("accent").filter(|_| paint.truecolor()) else {
        return SPIN
            .iter()
            .map(|spin| format!("{} {}", paint.fg("accent", spin), paint.fg("accent", label)))
            .collect();
    };
    let peak = mix(
        base,
        if paint.is_light() {
            [0, 0, 0]
        } else {
            [255, 255, 255]
        },
        0.6,
    );
    let chars: Vec<char> = label.chars().collect();
    let travel = chars.len() as f32 + 8.0;
    (0..SHIMMER_FRAMES)
        .map(|frame| {
            let center = frame as f32 * travel / SHIMMER_FRAMES as f32 - 4.0;
            let mut text = String::new();
            for (at, letter) in chars.iter().enumerate() {
                let near = (1.0 - (at as f32 - center).abs() / 3.0).clamp(0.0, 1.0);
                let eased = near * near * (3.0 - 2.0 * near);
                text.push_str(&rgb_fg(mix(base, peak, eased)));
                text.push(*letter);
            }
            format!(
                "{}{} {text}\x1b[39m",
                rgb_fg(base),
                SPIN[frame % SPIN.len()]
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_ext_sdk::style::strip;
    use serde_json::json;

    fn theme(mode: &str) -> Paint {
        let theme = json!({ "mode": mode, "appearance": "dark", "fg": { "accent": "\u{1b}[38;2;100;150;200m" }, "bg": {} });
        Paint::new(Some(serde_json::from_value(theme).unwrap()))
    }

    #[test]
    fn shimmer_frames_keep_the_label_and_cycle_the_spinner() {
        let frames = working_frames(&theme("truecolor"), "Thinking…", true);
        assert_eq!(frames.len(), SHIMMER_FRAMES);
        assert_eq!(strip(&frames[0]), "⠋ Thinking…");
        assert_eq!(strip(&frames[11]), "⠙ Thinking…");
        assert_ne!(frames[3], frames[9], "the band moves");
        assert_eq!(
            working_frames(&theme("256color"), "Run", true).len(),
            SPIN.len()
        );
        assert_eq!(
            strip(&working_frames(&theme("truecolor"), "Run", false)[0]),
            "● Run"
        );
    }

    #[test]
    fn tokens_fall_back_to_basic_colors() {
        let paint = Paint::default();
        assert_eq!(paint.fg("accent", "a"), "\x1b[36ma\x1b[39m");
        assert_eq!(paint.fg("muted", ""), "");
        assert_eq!(paint.bg("toolSuccessBg", "x"), "x");
        assert_eq!(
            theme("truecolor").fg("accent", "a"),
            "\x1b[38;2;100;150;200ma\x1b[39m"
        );
    }
}
