//! Panel lines for the questionnaire state at a given width.

use pi_ext_sdk::style::{self, Color};

use crate::config::{COLLAPSE_KEY_OFF, display_key};
use crate::layout::{boxed, columns, wrap, wrap_prefixed};
use crate::schema::{OTHER_LABEL, Question};
use crate::state::{Questionnaire, Row};

/// Side-by-side preview needs at least this many columns.
pub const SIDE_BY_SIDE_MIN_WIDTH: usize = 100;
const CURSOR: &str = "▏";

pub fn render(q: &Questionnaire, width: usize) -> Vec<String> {
    let width = width.max(20);
    let lines = if q.collapsed {
        vec![collapsed_line(q)]
    } else {
        let mut lines = Vec::new();
        if q.is_multi() {
            lines.push(tab_bar(q));
            lines.push(String::new());
        }
        if q.on_submit_tab() {
            lines.extend(submit_tab(q, width));
        } else if let Some(question) = q.question() {
            lines.extend(question_tab(q, question, width));
        }
        lines.push(String::new());
        lines.push(style::dim(&hints(q)));
        lines
    };
    lines
        .into_iter()
        .map(|line| style::truncate(&line, width))
        .collect()
}

fn collapsed_line(q: &Questionnaire) -> String {
    let key = display_key(&q.collapse_key);
    let header = q
        .question()
        .map_or("Submit", |question| question.header.as_str());
    style::dim(&format!("? {header} · {key} to expand · Esc to cancel"))
}

fn tab_bar(q: &Questionnaire) -> String {
    let mut chips: Vec<String> = q
        .questions
        .iter()
        .enumerate()
        .map(|(i, question)| {
            let mark = if q.answers.contains_key(&i) {
                "■"
            } else {
                "□"
            };
            (format!(" {mark} {} ", question.header), i)
        })
        .map(|(chip, i)| highlight(&chip, i == q.current_tab))
        .collect();
    let submit = highlight(" ✔ Submit ", q.on_submit_tab());
    chips.push(submit);
    format!("← {} →", chips.join(" "))
}

fn highlight(text: &str, active: bool) -> String {
    if active {
        style::inverse(text)
    } else {
        text.to_owned()
    }
}

fn question_tab(q: &Questionnaire, question: &Question, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if !q.is_multi() {
        lines.push(style::fg(Color::Cyan, &format!("[{}]", question.header)));
    }
    lines.extend(
        wrap(&question.question, width)
            .iter()
            .map(|l| style::bold(l)),
    );
    lines.push(String::new());
    let preview = question.has_preview() && !question.is_multi();
    if preview && width >= SIDE_BY_SIDE_MIN_WIDTH {
        let left_width = width * 2 / 5;
        let rows = option_lines(q, question, left_width);
        let pane = boxed(&preview_text(q, question), width - left_width - 2);
        lines.extend(columns(&rows, &pane, left_width, 2));
    } else {
        lines.extend(option_lines(q, question, width));
        if preview {
            lines.push(String::new());
            lines.extend(boxed(&preview_text(q, question), width));
        }
    }
    lines.extend(notes_lines(q, width, "Notes:"));
    lines
}

fn preview_text(q: &Questionnaire, question: &Question) -> Vec<String> {
    let preview = match q.current_row() {
        Some(Row::Option(i)) => question.options[i].preview(),
        _ => None,
    };
    let text = preview.unwrap_or("No preview available");
    text.split('\n').map(str::to_owned).collect()
}

fn with_cursor(text: &str) -> String {
    format!("{text}{CURSOR}")
}

fn option_lines(q: &Questionnaire, question: &Question, width: usize) -> Vec<String> {
    let multi = question.is_multi();
    let mut lines = Vec::new();
    for (index, row) in q.current_rows().iter().enumerate() {
        let focused = index == q.option_index;
        let pointer = if focused { "❯ " } else { "  " };
        let number = if row.numbered() {
            format!("{}. ", index + 1)
        } else {
            String::new()
        };
        let draft = if q.input_mode && focused {
            q.input.clone()
        } else {
            q.custom_draft(q.current_tab)
        };
        let (check, label, description) = match row {
            Row::Option(i) => {
                let option = &question.options[*i];
                let check = if q.checked.contains(i) {
                    "[✔] "
                } else {
                    "[ ] "
                };
                (
                    check,
                    option.label.clone(),
                    Some(option.description.as_str()),
                )
            }
            Row::Other => {
                let check = if draft.trim().is_empty() {
                    "[ ] "
                } else {
                    "[✔] "
                };
                let label = match (q.input_mode && focused, draft.is_empty()) {
                    (true, _) => with_cursor(&draft),
                    (false, true) => OTHER_LABEL.to_owned(),
                    (false, false) => draft.clone(),
                };
                (check, label, None)
            }
            Row::Next => ("", "→ Next".to_owned(), None),
        };
        let check = if multi { check } else { "" };
        let head = format!("{pointer}{number}{check}");
        let indent = " ".repeat(style::visible_width(&head));
        for (i, line) in wrap_prefixed(&label, width, &head, &indent)
            .into_iter()
            .enumerate()
        {
            let line = if focused && i == 0 {
                style::fg(Color::Cyan, &line)
            } else {
                line
            };
            lines.push(line);
        }
        if let Some(description) = description.filter(|d| !d.is_empty()) {
            let dim: Vec<String> = wrap_prefixed(description, width, &indent, &indent);
            lines.extend(dim.iter().map(|l| style::dim(l)));
        }
    }
    lines
}

fn notes_lines(q: &Questionnaire, width: usize, header: &str) -> Vec<String> {
    if q.notes_visible {
        let mut lines = vec![String::new(), style::bold(header)];
        lines.extend(wrap(&with_cursor(&q.notes_draft), width));
        return lines;
    }
    let note = q.note_for(q.current_tab);
    match note.lines().next() {
        Some(first) if !first.is_empty() => {
            let label = if q.on_submit_tab() {
                "Note:"
            } else {
                "Note for this answer:"
            };
            vec![String::new(), style::dim(&format!("{label} {first}"))]
        }
        _ => Vec::new(),
    }
}

fn submit_tab(q: &Questionnaire, width: usize) -> Vec<String> {
    let mut lines = vec![style::bold("Review your answers")];
    let mut missing = Vec::new();
    for (i, question) in q.questions.iter().enumerate() {
        match q.answers.get(&i) {
            Some(answer) => {
                let text = format!("{}: {}", question.header, answer.scalar());
                lines.extend(wrap_prefixed(&text, width, "● ", "  "));
                if let Some(note) = answer.notes.as_deref().and_then(|n| n.lines().next()) {
                    lines.push(style::dim(&format!("  Note for this answer: {note}")));
                }
            }
            None => {
                lines.push(style::dim(&format!("○ {}: not answered", question.header)));
                missing.push(question.header.clone());
            }
        }
    }
    lines.push(String::new());
    if missing.is_empty() {
        lines.push("Ready to submit your answers?".to_owned());
    } else {
        let warning = "⚠ Answer remaining questions before submitting:";
        lines.push(style::fg(Color::Yellow, warning));
        lines.extend(missing.iter().map(|header| format!("  - {header}")));
    }
    for (i, label) in ["Submit answers", "Cancel"].iter().enumerate() {
        let focused = q.submit_choice == i;
        let line = format!("{}{label}", if focused { "❯ " } else { "  " });
        lines.push(if focused {
            style::fg(Color::Cyan, &line)
        } else {
            line
        });
    }
    lines.extend(notes_lines(q, width, "Global note:"));
    lines
}

fn hints(q: &Questionnaire) -> String {
    let notes_key = display_key(&q.notes_key);
    let editing = q.notes_visible || q.input_mode;
    let mut parts = vec!["Enter to select", "↑/↓ to navigate"];
    let multi_select = q.question().is_some_and(Question::is_multi);
    if multi_select && !editing {
        parts.push("Space to toggle");
    }
    let notes = format!("{notes_key} to add notes");
    if !editing {
        parts.push(&notes);
    }
    if q.is_multi() && !editing {
        parts.push("Tab to switch questions");
    }
    let collapse = format!("{} to collapse", display_key(&q.collapse_key));
    if q.collapse_key != COLLAPSE_KEY_OFF {
        parts.push(&collapse);
    }
    parts.push("Esc to cancel");
    if editing {
        parts.push("Shift+Enter for newline");
    }
    if q.input_mode {
        parts.push("Ctrl+U to clear");
    }
    parts.join(" · ")
}
