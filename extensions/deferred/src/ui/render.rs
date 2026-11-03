//! Lines of the `/deferred` panel.

use pi_ext_sdk::style::{self, Color};

use super::rows::{Inventory, Row, RuleRow, Scope, Why};
use super::{Entry, Screen, TABS, Tab, Tone, View};
use crate::config::{Kind, Mode};
use crate::measure::{summary_line, wrap};

const MAX_VISIBLE: usize = 10;
/// Columns line up from this width.
const WIDE: usize = 72;
/// Lines around the list: rule, title, tabs, search, blanks, status, hints, rule.
const CHROME: usize = 9;

pub(super) fn accent(text: &str) -> String {
    style::fg(Color::Cyan, text)
}

pub(super) fn muted(text: &str) -> String {
    style::fg(Color::Gray, text)
}

pub(super) fn warning(text: &str) -> String {
    style::fg(Color::Yellow, text)
}

pub(super) fn glyph(mode: Mode) -> String {
    match mode {
        Mode::On => style::fg(Color::Green, "●"),
        Mode::Names => accent("◐"),
        Mode::Deferred => warning("○"),
        Mode::User => style::fg(Color::Magenta, "◇"),
        Mode::Off => style::fg(Color::Red, "✕"),
    }
}

pub(super) fn mode_word(mode: Mode, width: usize) -> String {
    let text = format!("{:<width$}", mode.as_str());
    match mode {
        Mode::On => style::fg(Color::Green, &text),
        Mode::Names => accent(&text),
        Mode::Deferred => warning(&text),
        Mode::User => style::fg(Color::Magenta, &text),
        Mode::Off => style::fg(Color::Red, &text),
    }
}

fn tab_label(tab: Tab, inv: &Inventory) -> String {
    match tab {
        Tab::Kind(Kind::Tool) => format!("Tools ({})", inv.count(Kind::Tool)),
        Tab::Kind(Kind::Skill) => format!("Skills ({})", inv.count(Kind::Skill)),
        Tab::Kind(Kind::Context) => format!("Context ({})", inv.count(Kind::Context)),
        Tab::Kind(Kind::Section) => format!("Sections ({})", inv.count(Kind::Section)),
        Tab::Rules => format!("Rules ({})", inv.rules.len()),
    }
}

pub(super) fn why_marker(row: &Row) -> String {
    match &row.why {
        Why::Default => String::new(),
        Why::Auto => style::dim(" auto"),
        Why::Pinned(_) => style::dim(" pinned"),
        Why::Undeclared(_) => style::dim(" undeclared"),
        Why::Rule {
            pattern,
            scope,
            repo,
            ..
        } => {
            let at = match repo {
                Some(repo) => format!("repo {repo}"),
                None => scope_word(scope.as_ref()).to_owned(),
            };
            match *pattern == row.name {
                true => style::dim(&format!(" [{at}]")),
                false => style::dim(&format!(" rule {pattern} [{at}]")),
            }
        }
    }
}

fn scope_word(scope: Option<&Scope>) -> &'static str {
    match scope {
        Some(Scope::Global) => "global",
        Some(Scope::Project) => "project",
        Some(Scope::Repo(_)) => "repo",
        None => "file",
    }
}

fn item_line(row: &Row, selected: bool, cols: Option<usize>) -> String {
    let pointer = if selected {
        accent("→ ")
    } else {
        "  ".to_owned()
    };
    let paint = |text: &str| {
        if selected {
            accent(text)
        } else {
            text.to_owned()
        }
    };
    let tokens = format!("~{} tok", row.tokens);
    let (name, mode, tokens) = match cols {
        Some(width) => (
            paint(&style::fit(&row.name, width)),
            mode_word(row.mode, 8),
            muted(&format!("{tokens:>9}")),
        ),
        None => (paint(&row.name), mode_word(row.mode, 0), muted(&tokens)),
    };
    let loaded = if row.loaded {
        style::fg(Color::Green, " loaded")
    } else {
        String::new()
    };
    let summary = summary_line(&row.description);
    let description = if summary.is_empty() {
        String::new()
    } else {
        style::dim(&format!("  {summary}"))
    };
    format!(
        "{pointer}{} {name} {mode} {tokens}{loaded}{}{description}",
        glyph(row.mode),
        why_marker(row)
    )
}

fn rule_line(rule: &RuleRow, selected: bool, cols: Option<usize>) -> String {
    let pointer = if selected {
        accent("→ ")
    } else {
        "  ".to_owned()
    };
    let paint = |text: &str| {
        if selected {
            accent(text)
        } else {
            text.to_owned()
        }
    };
    let (kind, pattern, mode) = match cols {
        Some(width) => (
            format!("{:<8}", rule.kind.key()),
            paint(&style::fit(&rule.pattern, width)),
            mode_word(rule.mode, 8),
        ),
        None => (
            rule.kind.key().to_owned(),
            paint(&rule.pattern),
            mode_word(rule.mode, 0),
        ),
    };
    let place = match (&rule.scope, &rule.repo) {
        (Some(scope), _) => scope.label(),
        (None, Some(repo)) => format!("{} [repo {repo}] (read-only)", rule.source),
        (None, None) => format!("{} (read-only)", rule.source),
    };
    let plural = if rule.matches == 1 { "" } else { "es" };
    let matches = muted(&format!(" {} match{plural}", rule.matches));
    format!(
        "{pointer}{} {} {pattern} {mode}{matches}{}",
        glyph(rule.mode),
        style::dim(&kind),
        style::dim(&format!("  {place}"))
    )
}

impl View {
    pub fn render(&self, inv: &Inventory, width: usize, height: Option<usize>) -> Vec<String> {
        let width = width.max(20);
        let rule = muted(&"─".repeat(width));
        let mut lines = vec![rule.clone()];
        let summary = muted(&format!(
            "  ~{} tokens kept out of the prompt{}",
            inv.saved(),
            if inv.auto { " · auto on" } else { "" }
        ));
        lines.push(format!(
            "{}{summary}",
            accent(&style::bold(" Deferred context"))
        ));
        let tabs: Vec<String> = TABS
            .iter()
            .enumerate()
            .map(|(i, tab)| match i == self.tab {
                true => accent(&style::bold(&format!("[{}]", tab_label(*tab, inv)))),
                false => muted(&tab_label(*tab, inv)),
            })
            .collect();
        lines.push(format!(" {}", tabs.join("  ")));
        lines.push(match self.query.is_empty() {
            true => muted(" › type to search"),
            false => format!(" › {}▏", self.query),
        });
        lines.push(String::new());
        let budget = height.map_or(MAX_VISIBLE, |h| h.saturating_sub(CHROME).clamp(3, 30));
        match &self.screen {
            Screen::List => lines.extend(self.list(inv, budget, width)),
            Screen::Menu { choice } => lines.extend(self.menu(inv, *choice, width)),
            Screen::Scope { change, choice } => {
                lines.push(accent(&style::bold(&format!(
                    " Save the rule for {} in",
                    change.pattern
                ))));
                let labels = self.scopes().into_iter().map(|scope| match scope {
                    Scope::Global => "Global settings: every project".to_owned(),
                    Scope::Project => "Project settings: this project only".to_owned(),
                    Scope::Repo(repo) => format!("Repo {repo}: global settings, this repo only"),
                });
                for (i, label) in labels.enumerate() {
                    lines.push(if i == *choice {
                        format!("{}{}", accent(" → "), accent(&label))
                    } else {
                        format!("   {label}")
                    });
                }
            }
        }
        lines.push(String::new());
        if let Some((tone, text)) = &self.status {
            for line in wrap(text, width.saturating_sub(3)) {
                lines.push(match tone {
                    Tone::Busy => accent(&format!(" ⋯ {line}")),
                    Tone::Warn => warning(&format!(" {line}")),
                    Tone::Info => muted(&format!(" {line}")),
                });
            }
        }
        lines.push(self.hints());
        lines.push(rule);
        if let Some(height) = height {
            lines.truncate(height.max(1));
        }
        lines
            .into_iter()
            .map(|line| style::truncate(&line, width))
            .collect()
    }

    fn list(&self, inv: &Inventory, budget: usize, width: usize) -> Vec<String> {
        let entries = self.entries(inv);
        if entries.is_empty() {
            let text = match (self.query.is_empty(), self.tab()) {
                (false, _) => " No matches".to_owned(),
                (true, Tab::Rules) => {
                    " No rules yet. Pick an item and press → or enter to add one.".to_owned()
                }
                (true, Tab::Kind(kind)) => format!(" No {}s in this session", kind.label()),
            };
            return vec![muted(&text)];
        }
        let visible = budget.min(entries.len());
        let start = self
            .selected
            .saturating_sub(visible / 2)
            .min(entries.len() - visible);
        let shown = &entries[start..start + visible];
        let longest = shown
            .iter()
            .map(|entry| match entry {
                Entry::Item(row) => style::visible_width(&row.name),
                Entry::Rule(rule) => style::visible_width(&rule.pattern),
            })
            .max()
            .unwrap_or(0);
        let cols = (width >= WIDE).then(|| longest.clamp(8, width / 3));
        let mut lines: Vec<String> = shown
            .iter()
            .enumerate()
            .map(|(i, entry)| match entry {
                Entry::Item(row) => item_line(row, start + i == self.selected, cols),
                Entry::Rule(rule) => rule_line(rule, start + i == self.selected, cols),
            })
            .collect();
        if entries.len() > visible {
            lines.push(muted(&format!(
                "  ({}/{})",
                self.selected + 1,
                entries.len()
            )));
        }
        lines
    }
}
