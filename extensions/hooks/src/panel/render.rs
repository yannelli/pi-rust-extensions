//! Lines of the `/hooks` panel: rule, title, tabs, search, body, status, hints, rule.

use pi_ext_sdk::style::{self, Color};

use super::details::wrap;
use super::{AddStep, Row, Screen, Setting, Tab, Tone, View};
use crate::manage::EVENTS;
use crate::model::Scope;
use crate::policy::ConflictMode;

/// Lines around the body: rule, title, tabs, search, two blanks, hints, rule.
const CHROME: usize = 8;
/// The body keeps this many lines before the chrome around it shrinks.
const MIN_BODY: usize = 5;

/// What a panel line is, so a short panel drops chrome before body lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Gap,
    Search,
    Title,
    Rule,
    Tabs,
    Notice,
    Status,
    Hints,
    Body,
}

/// Parts in the order a short panel drops them.
const DROP: [Part; 8] = [
    Part::Gap,
    Part::Search,
    Part::Rule,
    Part::Title,
    Part::Tabs,
    Part::Notice,
    Part::Status,
    Part::Hints,
];

pub fn accent(text: &str) -> String {
    style::fg(Color::Cyan, text)
}

pub fn muted(text: &str) -> String {
    style::fg(Color::Gray, text)
}

pub fn warning(text: &str) -> String {
    style::fg(Color::Yellow, text)
}

impl View {
    /// `height` is the lines the panel may use; `None` keeps the 12-row list window.
    pub fn render(&self, rows: &[Row], width: usize, height: Option<usize>) -> Vec<String> {
        let width = width.max(20);
        let rule = muted(&"─".repeat(width));
        let mut parts = vec![
            (Part::Rule, rule.clone()),
            (Part::Title, self.title(rows)),
            (Part::Tabs, self.tabs_line(rows)),
        ];
        if !self.project_trusted {
            let notice = warning(" Project not trusted: project hooks hidden");
            parts.push((Part::Notice, notice));
        }
        let search = match self.query.is_empty() {
            true => muted(" › type to search"),
            false => format!(" › {}▏", self.query),
        };
        parts.push((Part::Search, search));
        parts.push((Part::Gap, String::new()));
        let status = self.status_lines(width);
        let hints = hint_lines(&self.hints(), width);
        let chrome = CHROME - 1 + hints.len() + status.len() + usize::from(!self.project_trusted);
        let budget = height.map(|h| {
            let small = MIN_BODY.min(h.saturating_sub(1));
            h.saturating_sub(chrome).max(small).max(1)
        });
        let body = match &self.screen {
            Screen::List => self.list(rows, width, budget),
            Screen::Menu { choice } => self.menu(rows, Some(*choice), width),
            Screen::Confirm { hash } => self.confirm(rows, hash, width),
            Screen::Add(_) => self.add_lines(),
        };
        let body = fit_body(body, budget, self.focus(rows, width));
        parts.extend(body.into_iter().map(|line| (Part::Body, line)));
        parts.push((Part::Gap, String::new()));
        parts.extend(status.into_iter().map(|line| (Part::Status, line)));
        parts.extend(hints.into_iter().map(|line| (Part::Hints, line)));
        parts.push((Part::Rule, rule));
        if let Some(height) = height {
            for part in DROP {
                while parts.len() > height
                    && let Some(at) = parts.iter().rposition(|(p, _)| *p == part)
                {
                    parts.remove(at);
                }
            }
            parts.truncate(height.max(1));
        }
        parts
            .into_iter()
            .map(|(_, line)| style::truncate(&line, width))
            .collect()
    }

    fn title(&self, rows: &[Row]) -> String {
        let mut title = format!(" {}", accent(&style::bold("Hooks")));
        let plural = if rows.len() == 1 { "" } else { "s" };
        let mut summary = format!("  {} hook{plural}", rows.len());
        if self.options.conflicts != ConflictMode::Merge {
            let mode = self.options.conflicts.name();
            summary.push_str(&format!(" · conflicts: {mode}"));
        }
        title.push_str(&muted(&summary));
        if self.trust_all() {
            let flag = if self.trust_flag { " (flag)" } else { "" };
            title.push_str(&warning(&format!(" · trustAll on{flag}")));
        }
        title
    }

    fn tabs_line(&self, rows: &[Row]) -> String {
        let tabs: Vec<String> = Tab::ALL
            .iter()
            .map(|tab| {
                let count = match tab {
                    Tab::Settings => Setting::all().len(),
                    _ => rows.iter().filter(|r| tab.shows(r)).count(),
                };
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

    fn status_lines(&self, width: usize) -> Vec<String> {
        let Some((tone, text)) = &self.status else {
            return Vec::new();
        };
        wrap(text, width.saturating_sub(2))
            .into_iter()
            .map(|line| match tone {
                Tone::Warn => warning(&format!(" {line}")),
                Tone::Info => muted(&format!(" {line}")),
            })
            .collect()
    }

    /// The body line to keep in view when the body does not fit.
    fn focus(&self, rows: &[Row], width: usize) -> usize {
        match &self.screen {
            Screen::Menu { choice } => {
                let lines = self.menu(rows, Some(*choice), width).len();
                lines.saturating_sub(self.choice_count(rows).saturating_sub(*choice))
            }
            Screen::Confirm { .. } => usize::MAX,
            _ => 0,
        }
    }

    fn add_lines(&self) -> Vec<String> {
        let Screen::Add(form) = &self.screen else {
            return Vec::new();
        };
        let mut lines = vec![accent(&style::bold(" Add a command hook"))];
        let field = |label: &str, value: &str, active: bool| {
            let head = muted(&format!(" {label:<8}"));
            if active {
                format!("{head} {}", accent(&format!("› {value}▏")))
            } else {
                format!("{head}   {value}")
            }
        };
        let step = form.step;
        lines.push(field("event", EVENTS[form.event], step == AddStep::Event));
        if step == AddStep::Event {
            let around = [EVENTS.len() - 1, 1].map(|d| EVENTS[(form.event + d) % EVENTS.len()]);
            let around = format!("            ↑ {} · ↓ {}", around[0], around[1]);
            lines.push(muted(&around));
        }
        let matcher = match (step, form.matcher.is_empty()) {
            (AddStep::Event, _) | (AddStep::Matcher, true) => String::new(),
            (_, true) => "*".into(),
            (_, false) => form.matcher.clone(),
        };
        lines.push(field("matcher", &matcher, step == AddStep::Matcher));
        if step == AddStep::Matcher {
            let hint = "            a tool name, an a|b list, or a regex; empty matches all";
            lines.push(muted(hint));
        }
        lines.push(field("command", &form.command, step == AddStep::Command));
        let scopes: Vec<String> = self
            .scopes()
            .into_iter()
            .map(|scope| {
                let label = match scope {
                    Scope::Global => "global",
                    Scope::Project => "project",
                };
                if scope == form.scope {
                    accent(&format!("[{label}]"))
                } else {
                    muted(label)
                }
            })
            .collect();
        lines.push(format!(" {} {}", muted("save to "), scopes.join("  ")));
        lines
    }

    fn confirm(&self, rows: &[Row], hash: &str, width: usize) -> Vec<String> {
        let mut lines = self.menu(rows, None, width);
        if let Some(row) = rows.iter().find(|r| r.def.hash == hash) {
            let name = row.def.file.file_name().unwrap_or_default();
            let question = format!(
                "Remove {} {} from {}? Enter removes it; any other key cancels.",
                row.def.event,
                row.matcher(),
                name.to_string_lossy()
            );
            lines.push(String::new());
            let wrapped = wrap(&question, width.saturating_sub(2));
            lines.extend(wrapped.iter().map(|l| warning(&format!(" {l}"))));
        }
        lines
    }
}

/// Cuts `body` to `budget` lines around `focus`.
fn fit_body(body: Vec<String>, budget: Option<usize>, focus: usize) -> Vec<String> {
    match budget {
        Some(budget) if body.len() > budget => {
            let focus = focus.min(body.len() - 1);
            let start = focus.saturating_sub(budget / 2).min(body.len() - budget);
            body[start..start + budget].to_vec()
        }
        _ => body,
    }
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
        if style::visible_width(&next) > width && !line.is_empty() {
            lines.push(muted(&line));
            line = format!(" {hint}");
        } else {
            line = next;
        }
    }
    lines.push(muted(&line));
    lines
}
