//! Text for `/permissions`: the rule list and the explanation of a sample call.

use pi_ext_sdk::style;

use crate::evaluate::{Call, Verdict, evaluate};
use crate::load::Loaded;
use crate::rules::{Decision, Rule, Source};

pub const HELP: &str =
    "up/down move  a add  d remove  s sources  c scope  t test a command  q close";

/// Rules in display order (deny, ask, allow), one row per rule text and file.
pub fn rows(loaded: &Loaded) -> Vec<Rule> {
    let mut rows: Vec<Rule> = Vec::new();
    for decision in Decision::ALL {
        for rule in loaded.rules.iter().filter(|r| r.decision == decision) {
            let seen = rows
                .iter()
                .any(|r| r.text == rule.text && r.file == rule.file && r.decision == decision);
            if !seen {
                rows.push(rule.clone());
            }
        }
    }
    rows
}

pub fn header(loaded: &Loaded) -> Vec<String> {
    let trust = if loaded.trusted {
        "trusted"
    } else {
        "not trusted"
    };
    let sources: Vec<String> = Source::toggle_keys()
        .into_iter()
        .map(|key| format!("{key} {}", on_off(loaded.options.key_on(key))))
        .collect();
    let mut lines = vec![
        format!(
            "Permissions  new rules: {}  project: {trust}",
            loaded.options.scope.name()
        ),
        format!("Sources: {}", sources.join("  ")),
    ];
    lines.extend(loaded.info.iter().map(|i| format!("  {i}")));
    lines.extend(loaded.notes.iter().map(|n| format!("  ! {n}")));
    lines
}

pub fn listing(loaded: &Loaded, rows: &[Rule], selected: Option<usize>) -> Vec<String> {
    let mut lines = header(loaded);
    if selected.is_some() {
        lines.insert(1, HELP.to_owned());
    }
    if rows.is_empty() {
        lines.push("No rules found.".to_owned());
    }
    for (index, rule) in rows.iter().enumerate() {
        let marker = if selected == Some(index) { ">" } else { " " };
        let line = format!(
            " {marker} {:<5} {}  [{}] {}",
            rule.decision.name(),
            rule.text,
            rule.source.label(),
            rule.file.display()
        );
        lines.push(if selected == Some(index) {
            style::inverse(&line)
        } else {
            line
        });
    }
    lines
}

/// Why a call would be allowed, asked, or denied.
pub fn explain(loaded: &Loaded, call: &Call) -> Vec<String> {
    if let Some(reason) = loaded.blocked() {
        return vec![format!("{}: blocked ({reason})", call.tool)];
    }
    let verdict = evaluate(&loaded.rules, call);
    let guard = (verdict.decision != Some(Decision::Deny))
        .then(|| crate::fence::check(loaded, call, &verdict))
        .flatten();
    let first = guard.map_or_else(|| outcome(&verdict), |guard| guard.describe());
    let mut lines = vec![format!("{}: {first}", call.tool)];
    for (target, rule) in &verdict.targets {
        let why = match rule {
            Some(rule) => format!("{} by {}", rule.decision.name(), rule.describe()),
            None => "no rule".to_owned(),
        };
        lines.push(format!("  {}: {why}", target.describe()));
    }
    lines
}

fn outcome(verdict: &Verdict) -> String {
    match verdict.decision {
        Some(d) => d.name().to_owned(),
        None => "no decision (pi's normal behavior)".to_owned(),
    }
}

pub fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}
