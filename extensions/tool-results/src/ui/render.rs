//! Lines of the `/tool-results` panel.

use pi_ext_sdk::style::{self, Color};
use serde_json::Value;

use super::{Screen, TABS, Tab, View, patterns, rule_fields, rule_overrides};
use crate::config::{Config, FIELDS, Field, Kind};
use crate::lines::{group, short};
use crate::stats::{Stats, ToolStats, sum};

/// Lines under the list: a blank line, the status, and the hints.
const BELOW: usize = 3;

pub struct Inputs<'a> {
    pub config: &'a Config,
    pub stats: &'a Stats,
    /// A note under the title, such as the project file that overrides the global one.
    pub note: Option<&'a str>,
}

fn accent(text: &str) -> String {
    style::fg(Color::Cyan, text)
}

fn muted(text: &str) -> String {
    style::fg(Color::Gray, text)
}

fn value_text(field: &Field, value: &Value) -> String {
    match (field.kind, value) {
        (Kind::Bool, Value::Bool(true)) => style::fg(Color::Green, "on"),
        (Kind::Bool, Value::Bool(false)) => style::fg(Color::Red, "off"),
        (Kind::Num(_), Value::Number(n)) => group(n.as_u64().unwrap_or(0) as usize),
        (_, Value::String(s)) => accent(s),
        (_, other) => other.to_string(),
    }
}

fn pointer(selected: bool) -> String {
    if selected {
        accent("→ ")
    } else {
        "  ".to_owned()
    }
}

fn tabs(view: &View, inputs: &Inputs) -> String {
    let label = |tab: Tab| match tab {
        Tab::Settings => "Settings".to_owned(),
        Tab::Rules => format!("Rules ({})", patterns(inputs.config).len()),
        Tab::Stats => format!("Stats ({})", inputs.stats.total.len()),
    };
    let parts: Vec<String> = TABS
        .iter()
        .enumerate()
        .map(|(i, tab)| match i == view.tab {
            true => style::inverse(&format!(" {} ", label(*tab))),
            false => muted(&format!(" {} ", label(*tab))),
        })
        .collect();
    parts.join(" ")
}

fn settings_rows(view: &View, config: &Config) -> Vec<String> {
    FIELDS
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let label = format!("{:<36}", field.label);
            let value = value_text(field, &config.get(field.key));
            format!("{}{label}{value}", pointer(i == view.selected))
        })
        .collect()
}

fn summary(overrides: &serde_json::Map<String, Value>) -> String {
    let parts: Vec<String> = overrides
        .iter()
        .map(|(k, v)| match v {
            Value::Bool(true) => format!("{k} on"),
            Value::Bool(false) => format!("{k} off"),
            Value::String(s) => format!("{k}={s}"),
            other => format!("{k}={other}"),
        })
        .collect();
    if parts.is_empty() {
        "no overrides".into()
    } else {
        parts.join(", ")
    }
}

fn rule_rows(view: &View, config: &Config) -> Vec<String> {
    let mut rows: Vec<String> = patterns(config)
        .iter()
        .enumerate()
        .map(|(i, pattern)| {
            let (user, builtin) = rule_overrides(config, pattern);
            let mut merged = builtin.clone();
            merged.extend(user.clone());
            let tag = match (builtin.is_empty(), user.is_empty()) {
                (false, true) => muted(" built-in"),
                (false, false) => muted(" built-in + file"),
                _ => String::new(),
            };
            let name = format!("{:<20}", pattern);
            format!(
                "{}{name}{tag}  {}",
                pointer(i == view.selected),
                muted(&summary(&merged))
            )
        })
        .collect();
    let add = rows.len();
    rows.push(format!(
        "{}{}",
        pointer(view.selected == add),
        accent("+ Add a rule (tool name or glob such as mcp__*)")
    ));
    rows
}

fn rule_field_rows(view: &View, config: &Config, pattern: &str) -> Vec<String> {
    let (user, builtin) = rule_overrides(config, pattern);
    rule_fields()
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let label = format!("{:<36}", field.label);
            let value = match (user.get(field.key), builtin.get(field.key)) {
                (Some(v), _) => value_text(field, v),
                (None, Some(v)) => format!("{} {}", value_text(field, v), muted("built-in")),
                (None, None) => muted(&format!(
                    "inherit ({})",
                    style::strip(&value_text(field, &config.get(field.key)))
                )),
            };
            format!("{}{label}{value}", pointer(i == view.selected))
        })
        .collect()
}

fn stat_line(name: &str, session: Option<&ToolStats>, total: &ToolStats) -> String {
    let session_saved = session.map_or(0, ToolStats::saved) as usize;
    let mut line = format!(
        "{:<22}{:>7} calls {:>6} changed {:>5} stored  saved {:>6} now {:>7} total",
        name,
        group(total.calls as usize),
        group(total.changed as usize),
        group(total.truncated as usize),
        short(session_saved),
        short(total.saved() as usize),
    );
    if total.dry_saved > 0 {
        line.push_str(&muted(&format!(
            "  dry run {}",
            short(total.dry_saved as usize)
        )));
    }
    line
}

fn stats_rows(view: &View, stats: &Stats) -> Vec<String> {
    let mut tools: Vec<(&String, &ToolStats)> = stats.total.iter().collect();
    tools.sort_by_key(|(_, s)| std::cmp::Reverse(s.saved() + s.dry_saved));
    if tools.is_empty() {
        return vec![muted("  No tool results yet.")];
    }
    tools
        .iter()
        .enumerate()
        .map(|(i, (name, total))| {
            format!(
                "{}{}",
                pointer(i == view.selected),
                stat_line(name, stats.session.get(*name), total)
            )
        })
        .collect()
}

fn stats_footer(stats: &Stats) -> Vec<String> {
    let all = sum(&stats.total);
    let mut out = vec![
        String::new(),
        format!(
            "  {}",
            stat_line("All tools", Some(&sum(&stats.session)), &all)
        ),
    ];
    if let Some(last) = &stats.last {
        let mode = if last.dry {
            " (dry run, not applied)"
        } else {
            ""
        };
        out.push(muted(&format!(
            "  Last: {} {} → {} characters{mode} [{}]",
            last.tool,
            group(last.before),
            group(last.after),
            last.applied.join(", ")
        )));
    }
    out
}

/// Keeps `selected` in view in a window of `rows` lines.
fn window(lines: Vec<String>, selected: usize, rows: usize) -> Vec<String> {
    if lines.len() <= rows {
        return lines;
    }
    let start = selected.saturating_sub(rows / 2).min(lines.len() - rows);
    lines[start..start + rows].to_vec()
}

fn hints(view: &View) -> String {
    let hints: &[(&str, &str)] = match (&view.screen, view.tab()) {
        (Screen::Rule(_), _) => &[
            ("↑↓", "select"),
            ("←→ space", "change"),
            ("enter", "edit number"),
            ("esc", "back"),
        ],
        (Screen::List, Tab::Settings) => &[
            ("↑↓", "select"),
            ("←→ space", "change"),
            ("enter", "edit"),
            ("tab", "next tab"),
            ("esc", "close"),
        ],
        (Screen::List, Tab::Rules) => &[
            ("↑↓", "select"),
            ("enter", "open"),
            ("a", "add"),
            ("d", "delete"),
            ("tab", "next tab"),
            ("esc", "close"),
        ],
        (Screen::List, Tab::Stats) => {
            &[("r", "reset stats"), ("tab", "next tab"), ("esc", "close")]
        }
    };
    let parts: Vec<String> = hints
        .iter()
        .map(|(k, what)| format!("{} {}", style::dim(k), muted(what)))
        .collect();
    format!(" {}", parts.join(&style::dim(" · ")))
}

impl View {
    pub fn render(&self, inputs: &Inputs, width: usize, height: Option<usize>) -> Vec<String> {
        let mut out = vec![
            style::bold(" Tool results"),
            format!(" {}", tabs(self, inputs)),
        ];
        if let Some(note) = inputs.note {
            out.push(muted(&format!(" {note}")));
        }
        out.push(String::new());
        let (rows, extra) = match (&self.screen, self.tab()) {
            (Screen::Rule(pattern), _) => {
                out.push(format!(" Rule {}", accent(pattern)));
                (rule_field_rows(self, inputs.config, pattern), Vec::new())
            }
            (Screen::List, Tab::Settings) => (settings_rows(self, inputs.config), Vec::new()),
            (Screen::List, Tab::Rules) => (rule_rows(self, inputs.config), Vec::new()),
            (Screen::List, Tab::Stats) => {
                (stats_rows(self, inputs.stats), stats_footer(inputs.stats))
            }
        };
        let room = height.map_or(usize::MAX, |h| {
            h.saturating_sub(out.len() + BELOW + extra.len()).max(3)
        });
        out.extend(window(rows, self.selected, room));
        out.extend(extra);
        out.push(String::new());
        if let Some(status) = &self.status {
            out.push(style::fg(Color::Yellow, &format!(" {status}")));
        }
        out.push(hints(self));
        out.into_iter()
            .map(|line| style::truncate(&line, width))
            .collect()
    }
}
