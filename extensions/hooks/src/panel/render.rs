use pi_ext_sdk::style::{self, Color};

use super::details::{details_lines, wrap};
use super::{DetailAction, Group, Mode, Row, Tab, View};
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

/// How much chrome surrounds the body; `render` takes the first level that leaves room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chrome {
    Full,
    Tight,
    Minimal,
}

/// The list or details body shows at least this many lines before chrome shrinks.
const MIN_BODY: usize = 5;

impl View {
    /// `height` is the lines the panel may use; `None` keeps the 12-row list window.
    pub fn render(&self, rows: &[Row], width: usize, height: Option<usize>) -> Vec<String> {
        let width = width.max(20);
        let Some(height) = height else {
            return self.layout(rows, width, Chrome::Full, None);
        };
        let height = height.max(1);
        let levels = [Chrome::Full, Chrome::Tight, Chrome::Minimal];
        let level = levels
            .into_iter()
            .find(|&level| {
                let (head, foot) = self.chrome(rows, width, level);
                let needed = self.body(rows, width, None).len().min(MIN_BODY);
                head.len() + foot.len() + needed <= height
            })
            .unwrap_or(Chrome::Minimal);
        let mut lines = self.layout(rows, width, level, Some(height));
        lines.truncate(height);
        lines
    }

    fn layout(
        &self,
        rows: &[Row],
        width: usize,
        level: Chrome,
        height: Option<usize>,
    ) -> Vec<String> {
        let (mut lines, mut foot) = self.chrome(rows, width, level);
        if let Some(height) = height {
            while lines.len() + foot.len() >= height && !lines.is_empty() {
                lines.remove(0);
            }
            while lines.len() + foot.len() >= height && foot.pop().is_some() {}
        }
        let budget = height.map(|h| h.saturating_sub(lines.len() + foot.len()).max(1));
        lines.extend(self.body(rows, width, budget));
        lines.extend(foot);
        lines
            .into_iter()
            .map(|line| style::truncate(&line, width))
            .collect()
    }

    /// The lines above and below the body at `level`.
    fn chrome(&self, rows: &[Row], width: usize, level: Chrome) -> (Vec<String>, Vec<String>) {
        let full = level == Chrome::Full;
        let rule = muted(&"─".repeat(width));
        let mut head = Vec::new();
        let mut title = format!(" {}", accent(&style::bold("Hooks")));
        if self.trust_all {
            title.push_str(&warning("  trustAll on"));
        }
        if level == Chrome::Minimal {
            head.push(format!("{title} {}", self.tabs_line(rows)));
        } else {
            head.extend(full.then(|| rule.clone()));
            head.push(title);
            head.push(self.tabs_line(rows));
            if !self.project_trusted {
                head.push(warning(" Project not trusted: project hooks hidden"));
            }
        }
        if self.mode == Mode::Search || !self.query.is_empty() {
            let cursor = if self.mode == Mode::Search { "▏" } else { "" };
            head.push(format!(" {} {}{cursor}", muted("Search:"), self.query));
        }
        head.extend(full.then(String::new));
        let mut foot = Vec::new();
        if self.mode == Mode::AddScope {
            foot.extend(full.then(String::new));
            foot.push(warning(" Add a hook to: p project · g global"));
        }
        foot.extend(full.then(String::new));
        if let Some(status) = &self.status {
            let status = format!(" {status}");
            if full {
                foot.extend(wrap(&status, width).iter().map(|l| warning(l)));
            } else {
                foot.push(warning(&style::truncate(&status, width)));
            }
        }
        if full {
            foot.extend(hint_lines(&self.hints(), width));
        } else {
            foot.push(hint_line(&self.hints(), width));
        }
        foot.extend((level != Chrome::Minimal).then_some(rule));
        (head, foot)
    }

    /// The list or details lines, cut to `budget` lines around the cursor.
    fn body(&self, rows: &[Row], width: usize, budget: Option<usize>) -> Vec<String> {
        let (lines, focus) = match &self.mode {
            Mode::Details { hash, action } => {
                let lines = details_lines(rows, hash, Some(*action), width);
                let focus = lines.len().saturating_sub(DetailAction::ALL.len() - action);
                (lines, focus)
            }
            Mode::ConfirmRemove { hash } => {
                let mut lines = details_lines(rows, hash, None, width);
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
                let focus = lines.len().saturating_sub(1);
                (lines, focus)
            }
            Mode::AddScope | Mode::List | Mode::Search => {
                return self.list_lines(rows, width, budget);
            }
        };
        match budget {
            Some(budget) if lines.len() > budget => {
                let start = focus.saturating_sub(budget / 2).min(lines.len() - budget);
                lines[start..start + budget].to_vec()
            }
            _ => lines,
        }
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

    fn list_lines(&self, rows: &[Row], width: usize, budget: Option<usize>) -> Vec<String> {
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
        let shown: Vec<&Row> = visible.iter().map(|&i| &rows[i]).collect();
        let event_w = shown.iter().map(|r| r.def.event.len()).max().unwrap_or(0);
        let matcher_w = shown.iter().map(|r| r.matcher().len()).max().unwrap_or(0);
        let columns = (event_w.min(18), matcher_w.min(12));
        let groups: Vec<Group> = shown.iter().map(|r| r.group).collect();
        let max_rows = if budget.is_some() {
            usize::MAX
        } else {
            MAX_VISIBLE
        };
        let max_lines = budget.unwrap_or(usize::MAX);
        let Some((start, end)) = window(&groups, selected, max_rows, max_lines) else {
            return vec![row_line(shown[selected], true, width, columns)];
        };
        let mut lines = Vec::new();
        let mut group = None;
        for (position, row) in shown.iter().enumerate().take(end).skip(start) {
            if group != Some(row.group) {
                group = Some(row.group);
                let count = groups.iter().filter(|&&g| g == row.group).count();
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

/// Lines that rows `start..end` take: rows, group titles, and the `(n/m)` counter.
fn window_lines(groups: &[Group], start: usize, end: usize) -> usize {
    let titles = 1
        + (start + 1..end)
            .filter(|&i| groups[i] != groups[i - 1])
            .count();
    let counter = usize::from(start > 0 || end < groups.len());
    end - start + titles + counter
}

/// The rows around `selected` that fit `max_rows` and `max_lines`; grows down and up in turn.
/// `None` when the selected row with its group title and counter does not fit.
fn window(
    groups: &[Group],
    selected: usize,
    max_rows: usize,
    max_lines: usize,
) -> Option<(usize, usize)> {
    let fits = |start: usize, end: usize| {
        end <= groups.len()
            && end - start <= max_rows
            && window_lines(groups, start, end) <= max_lines
    };
    let (mut start, mut end) = (selected, selected + 1);
    if !fits(start, end) {
        return None;
    }
    let mut down = true;
    loop {
        let grown = [down, !down].into_iter().find_map(|down| {
            let next = if down {
                (start, end + 1)
            } else {
                (start.checked_sub(1)?, end)
            };
            fits(next.0, next.1).then_some((next, down))
        });
        let Some(((next_start, next_end), went_down)) = grown else {
            return Some((start, end));
        };
        (start, end, down) = (next_start, next_end, !went_down);
    }
}

/// One hint line: the hints from the start that fit, then the last hint (close or cancel).
fn hint_line(hints: &[&str], width: usize) -> String {
    let Some((last, rest)) = hints.split_last() else {
        return String::new();
    };
    let mut kept: Vec<&str> = Vec::new();
    for hint in rest {
        let line = [&kept[..], &[hint, last]].concat().join(" · ");
        if style::visible_width(&line) + 1 > width {
            break;
        }
        kept.push(hint);
    }
    kept.push(last);
    style::dim(&format!(" {}", kept.join(" · ")))
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
