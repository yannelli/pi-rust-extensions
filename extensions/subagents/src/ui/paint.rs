//! Colors from pi's theme, with basic ANSI colors when pi sends no theme, and line helpers.

use pi_ext_sdk::style;
use pi_ext_sdk::{Host, Theme};

#[derive(Clone, Debug, Default)]
pub struct Paint {
    theme: Option<Theme>,
}

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
                    "accent" => "36",
                    "success" => "32",
                    "error" => "31",
                    "warning" => "33",
                    "info" => "34",
                    "custom" => "35",
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

    /// A full-width selected row: pi's selection background, else bold, else inverse video.
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

    /// The bold title on the left and `right` (styled by the caller) on the right.
    pub fn header(&self, title: &str, right: &str, width: usize) -> String {
        let title = style::bold(&self.accent(title));
        let used = style::visible_width(&title) + style::visible_width(right) + 1;
        let gap = width.saturating_sub(used);
        style::truncate(&format!("{title}{}{right} ", " ".repeat(gap)), width)
    }

    /// A pointer row: `❯ text` on the selection background, else indented text.
    pub fn row(&self, text: &str, selected: bool, width: usize) -> String {
        if selected {
            self.selected(&format!("{} {text}", self.accent("❯")), width)
        } else {
            format!("  {text}")
        }
    }
}

/// The hint line: `key action · key action`.
pub fn hints(paint: &Paint, pairs: &[(&str, &str)], width: usize) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .map(|(key, action)| format!("{} {}", paint.fg("text", key), paint.dim(action)))
        .collect();
    style::truncate(&format!(" {}", parts.join(&paint.dim(" · "))), width)
}

/// Word wrap of plain text to `width` columns.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let next = style::visible_width(&line) + 1 + style::visible_width(word);
            if !line.is_empty() && next > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

/// The rows to show so that `selected` stays visible.
pub fn window(selected: usize, total: usize, rows: usize) -> std::ops::Range<usize> {
    if total <= rows {
        return 0..total;
    }
    let start = selected.saturating_sub(rows / 2).min(total - rows);
    start..start + rows
}

/// `provider/claude-sonnet-4-5` without the provider, for narrow columns.
pub fn short_model(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}
