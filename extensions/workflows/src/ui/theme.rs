//! Colors from pi's theme, with basic ANSI colors for hosts without `ui.getTheme`, and the
//! status glyphs.

use pi_ext_sdk::style;
use pi_ext_sdk::{Host, Theme};

use crate::runs::{RunStatus, StepStatus, now_ms};

#[derive(Clone, Debug, Default)]
pub struct Paint {
    theme: Option<Theme>,
}

const SPIN: [&str; 4] = ["◐", "◓", "◑", "◒"];

impl Paint {
    pub async fn load(host: &Host) -> Self {
        Self {
            theme: host.theme().await.ok().filter(|t| !t.fg.is_empty()),
        }
    }

    pub fn fg(&self, token: &str, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        match self.theme.as_ref().filter(|t| t.fg.contains_key(token)) {
            Some(theme) => theme.fg(token, text),
            None => {
                let code = match token {
                    "accent" | "mdHeading" => "36",
                    "success" => "32",
                    "error" => "31",
                    "warning" => "33",
                    "text" => "39",
                    _ => "90",
                };
                format!("\x1b[{code}m{text}\x1b[39m")
            }
        }
    }

    pub fn accent(&self, text: &str) -> String {
        self.fg("accent", text)
    }

    pub fn muted(&self, text: &str) -> String {
        self.fg("muted", text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.fg("dim", text)
    }

    /// A full-width selected row on pi's selection background. The terminal palette theme has
    /// no color for it and may not know the terminal's background, so the row is bold there;
    /// hosts without a theme get inverse video.
    pub fn selected(&self, line: &str, width: usize) -> String {
        let line = style::fit(line, width);
        match &self.theme {
            Some(theme) if theme.bg_rgb("selectedBg").is_some() => theme.bg("selectedBg", &line),
            Some(_) => style::bold(&line),
            None => style::inverse(&line),
        }
    }

    pub fn rule(&self, width: usize) -> String {
        self.fg("borderMuted", &"─".repeat(width))
    }

    /// A heading line: the bold title on the left, `right` (styled by the caller) on the right.
    pub fn header(&self, title: &str, right: &str, width: usize) -> String {
        let title = style::bold(&self.accent(title));
        let gap =
            width.saturating_sub(style::visible_width(&title) + style::visible_width(right) + 1);
        style::truncate(&format!("{title}{}{right} ", " ".repeat(gap)), width)
    }

    pub fn step_glyph(&self, status: StepStatus) -> String {
        match status {
            StepStatus::Pending => self.dim("○"),
            StepStatus::Running => self.accent(spinner()),
            StepStatus::Done => self.fg("success", "✓"),
            StepStatus::Failed => self.fg("error", "✗"),
        }
    }

    pub fn run_glyph(&self, status: RunStatus) -> String {
        match status {
            RunStatus::Running => self.accent(spinner()),
            RunStatus::Waiting => self.fg("warning", "◷"),
            RunStatus::Done => self.fg("success", "✓"),
            RunStatus::Failed => self.fg("error", "✗"),
            RunStatus::Cancelled => self.muted("■"),
            RunStatus::Interrupted => self.fg("warning", "◌"),
        }
    }
}

fn spinner() -> &'static str {
    SPIN[(now_ms() / 250 % SPIN.len() as u64) as usize]
}

/// Hint line: `key action · key action`.
pub fn hints(paint: &Paint, pairs: &[(&str, &str)], width: usize) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .map(|(key, action)| format!("{} {}", paint.fg("text", key), paint.dim(action)))
        .collect();
    style::truncate(&format!(" {}", parts.join(&paint.dim(" · "))), width)
}
