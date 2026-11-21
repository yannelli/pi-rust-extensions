//! The list body of the `/hooks` panel: the add row, hook rows under source titles, and settings.

use pi_ext_sdk::style::{self, Color};

use super::render::{accent, muted, warning};
use super::{Entry, Group, Row, Setting, View};
use crate::trust::Trust;

const MAX_VISIBLE: usize = 12;
/// Columns line up from this width.
const WIDE: usize = 72;

fn trust_color(trust: Trust, text: &str) -> String {
    match trust {
        Trust::Managed | Trust::All | Trust::Trusted => style::fg(Color::Green, text),
        Trust::Once => accent(text),
        Trust::Denied => style::fg(Color::Red, text),
        Trust::Untrusted => warning(text),
    }
}

fn trust_glyph(trust: Trust) -> &'static str {
    match trust {
        Trust::Managed => "M",
        Trust::All | Trust::Trusted => "✓",
        Trust::Once => "1",
        Trust::Denied => "✗",
        Trust::Untrusted => "?",
    }
}

impl View {
    pub(super) fn list(&self, rows: &[Row], width: usize, budget: Option<usize>) -> Vec<String> {
        let entries = self.entries(rows);
        if entries.is_empty() {
            return vec![muted(" No matches")];
        }
        let selected = self.selected.min(entries.len() - 1);
        let hooks = entries.iter().filter_map(|e| match e {
            Entry::Hook(row) => Some(*row),
            _ => None,
        });
        let (event_w, matcher_w) = hooks.fold((0, 0), |(e, m), r| {
            (e.max(r.def.event.len()), m.max(r.matcher().len()))
        });
        let columns = (event_w.min(18), matcher_w.min(12));
        let groups: Vec<Option<Group>> = entries
            .iter()
            .map(|e| match e {
                Entry::Hook(row) => Some(row.group),
                _ => None,
            })
            .collect();
        let max_rows = if budget.is_some() {
            usize::MAX
        } else {
            MAX_VISIBLE
        };
        let line = |i: usize| {
            let chosen = i == selected;
            match entries[i] {
                Entry::Add => add_row_line(chosen, rows.is_empty()),
                Entry::Hook(row) => row_line(row, chosen, width, columns),
                Entry::Setting(setting) => self.setting_line(setting, chosen, width),
            }
        };
        let max_lines = budget.unwrap_or(usize::MAX);
        let Some((start, end)) = window(&groups, selected, max_rows, max_lines) else {
            return vec![line(selected)];
        };
        let mut lines = Vec::new();
        let mut group = None;
        for (i, row_group) in groups.iter().enumerate().take(end).skip(start) {
            if let Some(g) = row_group.filter(|g| group != Some(*g)) {
                group = Some(g);
                let count = groups.iter().filter(|x| **x == Some(g)).count();
                lines.push(style::bold(&format!(" {} ({count})", g.title())));
            }
            lines.push(line(i));
        }
        if start > 0 || end < entries.len() {
            lines.push(muted(&format!("  ({}/{})", selected + 1, entries.len())));
        }
        lines
    }

    fn setting_line(&self, setting: Setting, selected: bool, width: usize) -> String {
        let pointer = if selected {
            accent("→ ")
        } else {
            "  ".into()
        };
        let label = style::fit(setting.label(), 25);
        let label = if selected { accent(&label) } else { label };
        let value = accent(&format!("{:<9}", setting.value_name(&self.options)));
        let about = if width >= WIDE {
            muted(&format!(" {}", setting.about()))
        } else {
            String::new()
        };
        format!(" {pointer}{label} {value}{about}")
    }
}

fn add_row_line(selected: bool, no_hooks: bool) -> String {
    let text = if no_hooks {
        "+ Add a hook… (no hooks found)"
    } else {
        "+ Add a hook…"
    };
    if selected {
        format!(" {}{}", accent("→ "), accent(text))
    } else {
        format!("   {}", muted(text))
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
    let mut command = match plugin {
        Some(id) => format!("{id}: {}", row.def.summary()),
        None => row.def.summary(),
    };
    if let Some(tag) = row.tag() {
        command = format!("[{tag}] {command}");
    }
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

/// Lines that entries `start..end` take: entries, group titles, and the `(n/m)` counter.
fn window_lines(groups: &[Option<Group>], start: usize, end: usize) -> usize {
    let titles = (start..end)
        .filter(|&i| groups[i].is_some() && (i == start || groups[i] != groups[i - 1]))
        .count();
    let counter = usize::from(start > 0 || end < groups.len());
    end - start + titles + counter
}

/// The entries around `selected` that fit `max_rows` and `max_lines`; grows down and up in turn.
/// `None` when the selected entry with its group title and counter does not fit.
fn window(
    groups: &[Option<Group>],
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
