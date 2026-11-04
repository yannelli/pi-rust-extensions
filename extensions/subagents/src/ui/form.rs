//! The side pane: the form with every field of the selected agent, its file, layers, prompt
//! line, and runs, or the dialog that is open.

use pi_ext_sdk::style;

use crate::agent::{PromptMode, Source};
use crate::render::agent_lines;
use crate::runs::{Run, Status};
use crate::ui::draft::{Draft, FIELDS, Field};
use crate::ui::flows::Modal;
use crate::ui::ops::{self, Entry};
use crate::ui::paint::{Paint, wrap};
use crate::ui::state::{Focus, State};
use crate::ui::view::{badge, inline, marker};

const LABEL: usize = 13;

pub fn detail_lines(
    state: &State,
    paint: &Paint,
    runs: &[Run],
    width: usize,
    rows: usize,
) -> Vec<String> {
    if let Some(modal) = state.modal.as_ref().filter(|m| inline(Some(m)).is_none()) {
        return modal_lines(modal, paint, width, rows);
    }
    let (Some(entry), Some(draft)) = (state.selected(), state.draft.as_ref()) else {
        let mut lines = vec![style::bold("New agent"), String::new()];
        let text = "Press enter to start from a template (read-only helper, worker, reviewer) or as a copy of an agent.";
        lines.extend(wrap(text, width).into_iter().map(|l| paint.muted(&l)));
        return lines;
    };
    let state_text = if draft.dirty() {
        paint.fg("warning", "● unsaved")
    } else {
        paint.muted("saved")
    };
    let title = format!(
        "{} {} {}",
        style::bold(&draft.name),
        badge(entry, paint),
        marker(entry, paint)
    );
    let mut lines = vec![paint_header(&title, &state_text, width)];
    let changed = draft.changed();
    let editing = inline(state.modal.as_ref());
    let form = state.focus == Focus::Form;
    for (i, field) in FIELDS.iter().enumerate() {
        let selected = form && i == state.field;
        let value = match editing {
            Some((f, edit)) if f == *field => edit.render(width.saturating_sub(LABEL + 4)),
            _ => value(*field, entry, draft, paint, selected),
        };
        let mark = if changed.contains(field) {
            paint.fg("warning", "•")
        } else {
            " ".into()
        };
        let line = format!("{mark}{}{value}", style::fit(field.label(), LABEL));
        lines.push(if selected {
            paint.selected(&format!("{} {line}", paint.accent("❯")), width)
        } else {
            format!("  {line}")
        });
    }
    lines.push(String::new());
    lines.extend(facts(state, entry, draft, paint, runs, width));
    let help = if form {
        state.field().help()
    } else {
        "Press enter to edit the fields of this agent."
    };
    lines.push(String::new());
    lines.extend(
        wrap(help, width.saturating_sub(2))
            .iter()
            .map(|l| paint.dim(&format!("  {l}"))),
    );
    for problem in draft.problems(&state.known_tools()) {
        let (glyph, token) = if problem.error {
            ("✗", "error")
        } else {
            ("⚠", "warning")
        };
        lines.push(paint.fg(token, &format!("  {glyph} {}", problem.text)));
    }
    let room = rows.saturating_sub(lines.len() + 2);
    if room >= 3 && !draft.values.prompt.trim().is_empty() {
        lines.push(String::new());
        lines.push(paint.dim("  Prompt"));
        let prompt = wrap(&draft.values.prompt, width.saturating_sub(6));
        let more = prompt.len().saturating_sub(room - 1);
        lines.extend(
            prompt
                .iter()
                .take(room - 1 - usize::from(more > 0))
                .map(|l| format!("    {}", paint.muted(l))),
        );
        if more > 0 {
            lines.push(paint.dim(&format!(
                "    … {} more lines · p opens the editor",
                more + 1
            )));
        }
    }
    lines
}

fn paint_header(left: &str, right: &str, width: usize) -> String {
    let gap = width.saturating_sub(style::visible_width(left) + style::visible_width(right) + 1);
    format!("{left}{}{right}", " ".repeat(gap))
}

fn value(field: Field, entry: &Entry, draft: &Draft, paint: &Paint, selected: bool) -> String {
    let v = &draft.values;
    let none = |text: &str| paint.dim(text);
    let choice = |text: String| {
        if selected {
            format!("‹ {text} ›")
        } else {
            text
        }
    };
    let list = |items: &[String]| {
        format!(
            "{} {}",
            items.join(", "),
            paint.dim(&format!("({})", items.len()))
        )
    };
    match field {
        Field::Name if entry.def.builtin => format!("{} {}", draft.name, none("· built-in name")),
        Field::Name => draft.name.clone(),
        Field::Description if v.description.trim().is_empty() => paint.fg("error", "empty"),
        Field::Description => v.description.lines().next().unwrap_or("").to_owned(),
        Field::Model => v
            .model
            .clone()
            .unwrap_or_else(|| none("inherit · session model and thinking")),
        Field::Thinking => choice(v.thinking.clone().unwrap_or_else(|| none("inherit"))),
        Field::Tools => match &v.tools {
            None => none("all · every tool except subagent"),
            Some(tools) if tools.is_empty() => paint.fg("warning", "none"),
            Some(tools) => list(tools),
        },
        Field::Disallowed if v.disallowed.is_empty() => none("none"),
        Field::Disallowed => list(&v.disallowed),
        Field::MaxTurns if v.max_turns.trim().is_empty() => none("no limit"),
        Field::MaxTurns => v.max_turns.clone(),
        Field::PromptMode => choice(match v.prompt_mode {
            PromptMode::Append => "append".to_owned(),
            PromptMode::Replace => format!("replace {}", none("· replaces pi's prompt")),
        }),
        Field::Exposure => choice(match v.exposure {
            Some(e) => e.as_str().to_owned(),
            None => none("inherit · pi's contextExposure setting"),
        }),
        Field::Prompt => {
            let first = v.prompt.lines().next().unwrap_or("").trim();
            match v.prompt.lines().count() {
                0 => paint.fg("warning", "empty"),
                1 => format!("{} {first}", none("1 line ·")),
                n => format!("{} {first}", none(&format!("{n} lines ·"))),
            }
        }
    }
}

/// File, layers, the system prompt line, and recent runs.
fn facts(
    state: &State,
    entry: &Entry,
    draft: &Draft,
    paint: &Paint,
    runs: &[Run],
    width: usize,
) -> Vec<String> {
    let layers = &state.setup.layers;
    let row =
        |label: &str, value: String| format!("  {}{value}", paint.dim(&style::fit(label, LABEL)));
    let mut lines = Vec::new();
    lines.push(row(
        "File",
        match entry.top() {
            Some(path) => tilde(path),
            None => paint.dim("none yet · the first save asks for global or project"),
        },
    ));
    let mut chain: Vec<String> = Vec::new();
    if entry.def.builtin {
        chain.push("built-in".into());
    }
    for file in &entry.def.files {
        chain.push(match ops::scope_of(layers, file) {
            Source::Project => "project".into(),
            _ => "global".into(),
        });
    }
    if let Some(plugin) = &entry.plugin {
        chain.push(format!("plugin {plugin}"));
    }
    lines.push(row("Layers", chain.join(" → ")));
    let mut def = entry.def.clone();
    def.description = draft.values.description.clone();
    def.name = draft.name.clone();
    let listed = agent_lines(std::slice::from_ref(&def))
        .trim_end()
        .to_owned();
    let note = match (state.setup.advertise, draft.values.exposure) {
        (false, _) => paint.dim(" · advertise is off"),
        (true, Some(crate::agent::Exposure::Deferred)) => {
            paint.dim(" · deferred: found with tool_search")
        }
        _ => String::new(),
    };
    let budget = width.saturating_sub(LABEL + 4 + style::visible_width(&note));
    lines.push(row(
        "Prompt line",
        format!("{}{note}", paint.muted(&style::truncate(&listed, budget))),
    ));
    let mine: Vec<&Run> = runs.iter().filter(|r| r.agent == draft.name).collect();
    let running = mine.iter().filter(|r| r.status == Status::Running).count();
    lines.push(row(
        "Runs",
        match mine.first() {
            None => paint.dim("none yet"),
            Some(last) => format!(
                "{} recent · {running} running · last {} {} {}",
                mine.len(),
                last.status.glyph(),
                last.status.as_str(),
                paint.dim("· u shows them")
            ),
        },
    ));
    lines
}

/// The path with the home directory as `~`.
fn tilde(path: &std::path::Path) -> String {
    let home = pi_ext_sdk::paths::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

pub fn modal_lines(modal: &Modal, paint: &Paint, width: usize, rows: usize) -> Vec<String> {
    match modal {
        Modal::Pick(picker, _) => picker.lines(paint, width, rows),
        Modal::Text(text) => {
            let mut lines = vec![style::bold(&text.title), String::new()];
            lines.push(format!(
                "  {} {}",
                paint.accent("›"),
                text.edit.render(width.saturating_sub(6))
            ));
            lines.push(String::new());
            lines.extend(
                wrap(&text.help, width.saturating_sub(2))
                    .iter()
                    .map(|l| paint.dim(&format!("  {l}"))),
            );
            lines
        }
        Modal::Choice(choice) => {
            let mut lines = vec![style::bold(&choice.title), String::new()];
            let label_width = choice
                .options
                .iter()
                .map(|(l, _)| l.chars().count())
                .max()
                .unwrap_or(0);
            for (i, (label, note)) in choice.options.iter().enumerate() {
                let label = style::bold(&style::fit(label, label_width));
                let text = format!(
                    "{} {label}  {}",
                    paint.dim(&(i + 1).to_string()),
                    paint.muted(note)
                );
                lines.push(paint.row(&text, i == choice.row, width));
            }
            lines
        }
    }
}
