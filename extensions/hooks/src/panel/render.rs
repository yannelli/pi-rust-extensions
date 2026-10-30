use pi_ext_sdk::style::{self, Color};

use super::details::{details_lines, wrap};
use super::{Mode, Row, Tab, View};
use crate::trust::Trust;

const MAX_VISIBLE: usize = 12;
const WIDE: usize = 72;

pub fn accent(text: &str) -> String {
    style::fg(Color::Cyan, text)
}

pub fn muted(text: &str) -> String {
    style::fg(Color::Gray, text)
}

fn warning(text: &str) -> String {
    style::fg(Color::Yellow, text)
}

fn trust_color(trust: Trust, text: &str) -> String {
    match trust {
        Trust::All | Trust::Trusted => style::fg(Color::Green, text),
        Trust::Once => accent(text),
        Trust::Denied => style::fg(Color::Red, text),
        Trust::Untrusted => warning(text),
    }
}

fn trust_glyph(trust: Trust) -> &'static str {
    match trust {
        Trust::All | Trust::Trusted => "✓",
        Trust::Once => "1",
        Trust::Denied => "✗",
        Trust::Untrusted => "?",
    }
}

impl View {
    pub fn render(&self, rows: &[Row], width: usize) -> Vec<String> {
        let width = width.max(20);
        let mut lines = vec![muted(&"─".repeat(width))];
        let mut title = format!(" {}", accent(&style::bold("Hooks")));
        if self.trust_all {
            title.push_str(&warning("  trustAll on"));
        }
        lines.push(title);
        lines.push(self.tabs_line(rows));
        if !self.project_trusted {
            lines.push(warning(" Project not trusted: project hooks hidden"));
        }
        if self.mode == Mode::Search || !self.query.is_empty() {
            let cursor = if self.mode == Mode::Search { "▏" } else { "" };
            lines.push(format!(" {} {}{cursor}", muted("Search:"), self.query));
        }
        lines.push(String::new());
        match &self.mode {
            Mode::Details { hash, action } => {
                lines.extend(details_lines(rows, hash, Some(*action), width));
            }
            Mode::ConfirmRemove { hash } => {
                lines.extend(details_lines(rows, hash, None, width));
                if let Some(row) = rows.iter().find(|r| &r.def.hash == hash) {
                    let name = row.def.file.file_name().unwrap_or_default();
                    let prompt = format!(
                        " Remove {} {} from {}? y confirm · n cancel",
                        row.def.event,
                        row.matcher(),
                        name.to_string_lossy()
                    );
                    lines.push(String::new());
                    lines.extend(wrap(&prompt, width).iter().map(|l| warning(l)));
                }
            }
            Mode::AddScope => {
                lines.extend(self.list_lines(rows, width));
                lines.push(String::new());
                lines.push(warning(" Add a hook to: p project · g global"));
            }
            Mode::List | Mode::Search => lines.extend(self.list_lines(rows, width)),
        }
        lines.push(String::new());
        if let Some(status) = &self.status {
            let status = wrap(&format!(" {status}"), width);
            lines.extend(status.iter().map(|l| warning(l)));
        }
        lines.extend(hint_lines(&self.hints(), width));
        lines.push(muted(&"─".repeat(width)));
        lines
            .into_iter()
            .map(|line| style::truncate(&line, width))
            .collect()
    }

    fn tabs_line(&self, rows: &[Row]) -> String {
        let tabs: Vec<String> = Tab::ALL
            .iter()
            .map(|tab| {
                let count = rows.iter().filter(|r| tab.shows(r)).count();
                let label = format!("{} ({count})", tab.title());
                if *tab == self.tab {
                    accent(&style::bold(&format!("[{label}]")))
                } else {
                    muted(&label)
                }
            })
            .collect();
        format!(" {}", tabs.join("  "))
    }

    fn list_lines(&self, rows: &[Row], width: usize) -> Vec<String> {
        let visible = self.visible(rows);
        if visible.is_empty() {
            let text = if rows.is_empty() {
                " No hooks found. Press a to add one."
            } else {
                " No matching hooks"
            };
            return vec![muted(text)];
        }
        let selected = self.selected.min(visible.len() - 1);
        let start = selected
            .saturating_sub(MAX_VISIBLE / 2)
            .min(visible.len().saturating_sub(MAX_VISIBLE));
        let end = (start + MAX_VISIBLE).min(visible.len());
        let shown: Vec<&Row> = visible.iter().map(|&i| &rows[i]).collect();
        let event_w = shown.iter().map(|r| r.def.event.len()).max().unwrap_or(0);
        let matcher_w = shown.iter().map(|r| r.matcher().len()).max().unwrap_or(0);
        let columns = (event_w.min(18), matcher_w.min(12));
        let mut lines = Vec::new();
        let mut group = None;
        for (position, row) in shown.iter().enumerate().take(end).skip(start) {
            if group != Some(row.group) {
                group = Some(row.group);
                let count = shown.iter().filter(|r| r.group == row.group).count();
                lines.push(style::bold(&format!(" {} ({count})", row.group.title())));
            }
            lines.push(row_line(row, position == selected, width, columns));
        }
        if start > 0 || end < visible.len() {
            lines.push(muted(&format!("  ({}/{})", selected + 1, visible.len())));
        }
        lines
    }

    fn hints(&self) -> Vec<&'static str> {
        match self.mode {
            Mode::Search => vec!["type to filter", "↑↓ move", "enter done", "esc clear"],
            Mode::Details { .. } => vec![
                "↑↓ choose",
                "enter run",
                "space toggle",
                "d remove",
                "esc back",
            ],
            Mode::ConfirmRemove { .. } => vec!["y remove", "n cancel"],
            Mode::AddScope => vec!["p project", "g global", "esc cancel"],
            Mode::List => {
                let close = if self.query.is_empty() {
                    "esc close"
                } else {
                    "esc clear search"
                };
                vec![
                    "↑↓ move",
                    "space toggle",
                    "enter details",
                    "d remove",
                    "a add",
                    "/ search",
                    "tab filter",
                    "s sources",
                    "c load scope",
                    "t trustAll",
                    close,
                ]
            }
        }
    }
}

fn row_line(row: &Row, selected: bool, width: usize, columns: (usize, usize)) -> String {
    let pointer = if selected {
        accent("→ ")
    } else {
        "  ".into()
    };
    let glyph = if row.enabled {
        style::fg(Color::Green, "●")
    } else {
        muted("○")
    };
    let text = |s: &str| {
        if !row.enabled {
            muted(s)
        } else if selected {
            accent(s)
        } else {
            s.to_owned()
        }
    };
    let plugin = row.def.plugin.as_ref().and_then(|p| p.id.as_deref());
    let command = match plugin {
        Some(id) => format!("{id}: {}", row.def.summary()),
        None => row.def.summary(),
    };
    let line = if width >= WIDE {
        let (event_w, matcher_w) = columns;
        format!(
            " {pointer}{glyph} {} {} {} {} {}",
            text(&style::fit(&row.def.event, event_w)),
            text(&style::fit(row.matcher(), matcher_w)),
            muted(&style::fit(row.scope_label(), 7)),
            trust_color(row.trust, &style::fit(row.trust.name(), 9)),
            text(&command)
        )
    } else {
        let scope = row.scope_label()[..1].to_uppercase();
        format!(
            " {pointer}{glyph} {} {} {}{} {}",
            text(&row.def.event),
            text(row.matcher()),
            muted(&scope),
            trust_color(row.trust, trust_glyph(row.trust)),
            text(&command)
        )
    };
    style::truncate(&line, width)
}

/// Joins hints with " · " and starts a new line before one would overflow `width`.
fn hint_lines(hints: &[&str], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for hint in hints {
        let next = if line.is_empty() {
            format!(" {hint}")
        } else {
            format!("{line} · {hint}")
        };
        if !line.is_empty() && style::visible_width(&next) > width {
            lines.push(style::dim(&line));
            line = format!(" {hint}");
        } else {
            line = next;
        }
    }
    lines.push(style::dim(&line));
    lines
}
