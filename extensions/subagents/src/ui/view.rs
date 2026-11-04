//! Drawing the panel: the header with tabs, the agent list, the form or a dialog, the status
//! line, and the key hints.

use pi_ext_sdk::style;

use crate::agent::Source;
use crate::runs::{Run, Status};
use crate::ui::draft::{Draft, FIELDS, Field};
use crate::ui::flows::{Modal, TextThen};
use crate::ui::form::detail_lines;
use crate::ui::ops::Entry;
use crate::ui::paint::{Paint, hints, short_model, window};
use crate::ui::state::{Focus, Level, State, Tab};

/// Wide terminals show the list and the form side by side.
const WIDE: usize = 100;

pub fn render(
    state: &State,
    paint: &Paint,
    runs: &[Run],
    width: usize,
    rows: usize,
) -> Vec<String> {
    let width = width.max(40);
    let mut lines = vec![header(state, paint, runs, width), paint.rule(width)];
    let body = rows.saturating_sub(5).max(10);
    match state.tab {
        Tab::Runs => {
            let shown = state.shown_runs(runs);
            let scope = match &state.runs_agent {
                Some(agent) => format!("Runs of {agent}"),
                None => "Runs of all agents".to_owned(),
            };
            lines.push(style::bold(&scope));
            let view = state.runs.render(&shown, width);
            lines.extend(view.into_iter().skip(2).take(body.saturating_sub(1)));
        }
        Tab::Agents if width >= WIDE => {
            let left = (width * 2 / 5).clamp(34, 56);
            let right = width - left - 3;
            let list = list_lines(state, paint, left, body);
            let detail = detail_lines(state, paint, runs, right, body);
            for i in 0..list.len().max(detail.len()).min(body) {
                let l = style::fit(list.get(i).map_or("", String::as_str), left);
                let r = detail.get(i).cloned().unwrap_or_default();
                lines.push(format!("{l}{}{r}", paint.dim(" │ ")));
            }
        }
        Tab::Agents => {
            let list_rows = (body / 3).max(5);
            lines.extend(list_lines(state, paint, width, list_rows));
            lines.push(paint.rule(width));
            let detail = detail_lines(state, paint, runs, width, body - list_rows);
            lines.extend(detail.into_iter().take(body - list_rows));
        }
    }
    lines.push(paint.rule(width));
    lines.push(status_line(state, paint, width));
    lines.push(hints(paint, &key_hints(state), width));
    lines
        .into_iter()
        .map(|l| style::truncate(&l, width))
        .collect()
}

fn header(state: &State, paint: &Paint, runs: &[Run], width: usize) -> String {
    let running = runs.iter().filter(|r| r.status == Status::Running).count();
    let tab = |label: &str, on: bool| {
        if on {
            style::bold(&paint.accent(&format!("[{label}]")))
        } else {
            paint.muted(label)
        }
    };
    let runs_label = if running > 0 {
        format!("Runs ({running} running)")
    } else {
        "Runs".to_owned()
    };
    let tabs = format!(
        "{}  {}",
        tab(
            &format!("Agents ({})", state.entries.len()),
            state.tab == Tab::Agents
        ),
        tab(&runs_label, state.tab == Tab::Runs)
    );
    paint.header("Subagents", &tabs, width)
}

/// The source badge with its color: built-in, global, project, or the plugin.
pub fn badge(entry: &Entry, paint: &Paint) -> String {
    let text = entry.badge();
    match (&entry.plugin, entry.origin) {
        (Some(_), _) => paint.fg("custom", &text),
        (None, Source::Project) => paint.fg("success", &text),
        (None, Source::Global) => paint.fg("info", &text),
        (None, Source::Builtin) => paint.muted(&text),
    }
}

/// `✎ <scope>` when a file in that scope overrides the built-in or a lower file.
pub fn marker(entry: &Entry, paint: &Paint) -> String {
    match entry.override_scope() {
        Some(scope) => paint.fg("warning", &format!("✎ {}", scope.label())),
        None => String::new(),
    }
}

fn list_lines(state: &State, paint: &Paint, width: usize, rows: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(search) = &state.searching {
        let input = search.render(width.saturating_sub(6));
        lines.push(format!("  {} {input}", paint.accent("⌕")));
    } else if !state.filter.is_empty() {
        lines.push(paint.muted(&format!("  ⌕ {} · esc clears", state.filter)));
    }
    let visible = state.visible();
    let name_width = visible
        .iter()
        .map(|&i| state.entries[i].def.name.chars().count());
    let name_width = name_width.max().unwrap_or(8).clamp(8, 22);
    let badge_width = visible
        .iter()
        .map(|&i| state.entries[i].badge().chars().count());
    let badge_width = badge_width.max().unwrap_or(8).clamp(8, 18);
    let total = visible.len() + 1;
    let list_focus = state.focus == Focus::List && state.tab == Tab::Agents;
    for row in window(state.row, total, rows.saturating_sub(lines.len()).max(3)) {
        let text = match visible.get(row) {
            Some(&i) => row_text(state, &state.entries[i], paint, name_width, badge_width),
            None => paint.accent("+ New agent"),
        };
        let selected = row == state.row;
        lines.push(match (selected, list_focus) {
            (true, true) => paint.row(&text, true, width),
            (true, false) => format!("{} {text}", paint.accent("›")),
            _ => format!("  {text}"),
        });
    }
    if visible.is_empty() && !state.filter.is_empty() {
        lines.insert(lines.len() - 1, paint.muted("  No agent matches."));
    }
    lines
}

fn row_text(state: &State, entry: &Entry, paint: &Paint, name_w: usize, badge_w: usize) -> String {
    let def = &entry.def;
    let dirty = state.dirty() && state.draft.as_ref().is_some_and(|d| d.name == def.name);
    let name = style::fit(&def.name, name_w);
    let name = if dirty {
        paint.fg("warning", &name)
    } else {
        style::bold(&name)
    };
    let badge = style::fit(&badge(entry, paint), badge_w);
    let model = def
        .model
        .as_deref()
        .map_or_else(|| paint.dim("session"), |m| short_model(m).to_owned());
    let exposure = def.exposure.map_or(String::new(), |e| {
        paint.muted(&format!(" · {}", e.as_str()))
    });
    let marker = style::fit(&marker(entry, paint), 9);
    format!("{name} {badge} {marker} {model}{exposure}")
}

pub fn status_line(state: &State, paint: &Paint, width: usize) -> String {
    let text = match &state.message {
        Some((Level::Ok, text)) => paint.fg("success", &format!(" ✓ {text}")),
        Some((Level::Error, text)) => paint.fg("error", &format!(" ✗ {text}")),
        Some((Level::Info, text)) => paint.accent(&format!(" • {text}")),
        None if state.tab == Tab::Runs => String::new(),
        None if state.modal.is_some() && inline(state.modal.as_ref()).is_none() => String::new(),
        None => draft_status(state.draft.as_ref(), &state.known_tools(), paint),
    };
    style::truncate(&text, width)
}

fn draft_status(draft: Option<&Draft>, known: &[String], paint: &Paint) -> String {
    let Some(draft) = draft else {
        return String::new();
    };
    let problems = draft.problems(known);
    let more = match problems.len() {
        0 | 1 => String::new(),
        n => format!(" (+{} more)", n - 1),
    };
    match problems.first() {
        Some(p) if p.error => paint.fg("error", &format!(" ✗ {}{more}", p.text)),
        Some(p) => paint.fg("warning", &format!(" ⚠ {}{more}", p.text)),
        None if draft.dirty() => paint.fg("warning", " ● unsaved changes · s saves"),
        None => paint.fg("success", " ✓ valid"),
    }
}

fn key_hints(state: &State) -> Vec<(&'static str, &'static str)> {
    if let Some(modal) = &state.modal {
        return match modal {
            Modal::Text(_) => vec![("enter", "done"), ("esc", "cancel"), ("ctrl+u", "clear")],
            Modal::Choice(_) => vec![
                ("↑↓", "move"),
                ("enter", "choose"),
                ("1-9", "pick"),
                ("esc", "cancel"),
            ],
            Modal::Pick(p, _) if p.multi => vec![
                ("type", "search"),
                ("space", "toggle"),
                ("ctrl+a", "toggle shown"),
                ("enter", "done"),
                ("esc", "clear search or cancel"),
            ],
            Modal::Pick(..) => vec![
                ("type", "search"),
                ("↑↓", "move"),
                ("enter", "choose"),
                ("esc", "clear search or cancel"),
            ],
        };
    }
    match (state.tab, state.focus) {
        (Tab::Runs, _) if state.runs.detail.is_some() => {
            vec![
                ("↑↓", "scroll"),
                ("pgup/pgdn", "page"),
                ("c", "cancel run"),
                ("esc", "back"),
            ]
        }
        (Tab::Runs, _) => {
            let mut keys = vec![("↑↓", "select"), ("enter", "output"), ("c", "cancel run")];
            if state.runs_agent.is_some() {
                keys.extend([("a", "all agents"), ("esc", "agents")]);
            }
            keys.extend([("tab", "agents"), ("q", "close")]);
            keys
        }
        _ if state.searching.is_some() => {
            vec![
                ("type", "filter"),
                ("↑↓", "move"),
                ("enter", "keep"),
                ("esc", "clear"),
            ]
        }
        (Tab::Agents, Focus::List) => vec![
            ("↑↓", "move"),
            ("enter", "edit"),
            ("n", "new"),
            ("c", "copy"),
            ("r", "rename"),
            ("m", "move scope"),
            ("d", "delete/reset"),
            ("f", "file"),
            ("u", "runs"),
            ("/", "search"),
            ("tab", "runs"),
            ("q", "close"),
        ],
        (Tab::Agents, Focus::Form) => {
            let field = FIELDS[state.field.min(FIELDS.len() - 1)];
            let mut keys = vec![("↑↓", "field")];
            keys.push(match field {
                f if f.cycles() => ("←→", "change"),
                Field::Prompt => ("enter", "open editor"),
                Field::Model | Field::Tools | Field::Disallowed => ("enter", "pick"),
                _ => ("enter", "edit"),
            });
            keys.extend([
                ("x", "default"),
                ("p", "prompt"),
                ("s", "save"),
                ("esc", "list"),
                ("q", "close"),
            ]);
            keys
        }
    }
}

/// Whether the modal draws in the form row instead of the side pane.
pub fn inline(modal: Option<&Modal>) -> Option<(Field, &crate::ui::line::LineEdit)> {
    match modal {
        Some(Modal::Text(text)) => match text.then {
            TextThen::Field(field) => Some((field, &text.edit)),
            _ => None,
        },
        _ => None,
    }
}
