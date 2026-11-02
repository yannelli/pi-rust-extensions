//! The actions pane of the `/deferred` panel: details of one entry and what can change.

use pi_ext_sdk::style;

use super::render::{accent, mode_word, muted, why_marker};
use super::rows::{Inventory, Row, Why};
use super::{Choice, Entry, View};
use crate::config::Mode;
use crate::measure::wrap;

impl View {
    pub(super) fn menu(&self, inv: &Inventory, choice: usize, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let (current, title) = match self.entries(inv).get(self.selected) {
            Some(Entry::Item(row)) => {
                lines.push(format!(
                    " {} {}{}",
                    accent(&style::bold(&row.name)),
                    mode_word(row.mode, 0),
                    why_marker(row)
                ));
                lines.extend(
                    wrap(&row.description, width.saturating_sub(2))
                        .into_iter()
                        .filter(|l| !l.is_empty())
                        .map(|l| format!(" {l}")),
                );
                lines.push(style::dim(&format!(" Kind: {}", row.kind.label())));
                lines.push(style::dim(&format!(" Mode: {}", explain(row))));
                lines.push(style::dim(&format!(
                    " Estimated cost when loaded: ~{} tokens",
                    row.tokens
                )));
                lines.extend(row.details.iter().map(|d| style::dim(&format!(" {d}"))));
                (row.mode, row.name.clone())
            }
            Some(Entry::Rule(rule)) => {
                lines.push(format!(
                    " {} {} {}",
                    accent(&style::bold(&rule.pattern)),
                    style::dim(rule.kind.key()),
                    mode_word(rule.mode, 0)
                ));
                lines.push(style::dim(&format!(
                    " From: {}",
                    rule.scope.map_or(rule.source.as_str(), |s| s.label())
                )));
                lines.push(style::dim(&format!(
                    " Matches {} item(s) now",
                    rule.matches
                )));
                (rule.mode, rule.pattern.clone())
            }
            None => return lines,
        };
        lines.push(String::new());
        lines.push(muted(" Actions"));
        for (i, item) in self.choices(inv).iter().enumerate() {
            let label = match item {
                Choice::Set(mode) => format!(
                    "{}{}",
                    choice_label(*mode),
                    if *mode == current { " (current)" } else { "" }
                ),
                Choice::Remove => format!("Remove the rule for {title}"),
                Choice::Unload => "Unload: forget that the model loaded it".to_owned(),
            };
            lines.push(if i == choice {
                format!("{}{}", accent(" → "), accent(&label))
            } else {
                format!("   {label}")
            });
        }
        lines
    }
}

fn choice_label(mode: Mode) -> &'static str {
    match mode {
        Mode::On => "Load normally (on)",
        Mode::Names => "List the name only (names)",
        Mode::Deferred => "Searchable, not listed (deferred)",
        Mode::Off => "Disable (off)",
    }
}

fn explain(row: &Row) -> String {
    let mode = row.mode.as_str();
    match &row.why {
        Why::Default => format!("{mode} (no rule matches)"),
        Why::Auto => format!("{mode} (auto: extension and MCP tools pass the context share)"),
        Why::Pinned(reason) => format!("{mode} ({reason})"),
        Why::Rule {
            pattern,
            source,
            scope,
        } => format!(
            "{mode} (rule `{pattern}` in {})",
            scope.map_or(source.as_str(), |s| s.label())
        ),
    }
}
