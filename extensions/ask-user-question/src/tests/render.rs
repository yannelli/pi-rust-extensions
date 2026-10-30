use pi_ext_sdk::style::{strip, visible_width};
use serde_json::json;

use super::{questions, two_questions};
use crate::render::render;
use crate::state::Questionnaire;

fn plain(q: &Questionnaire, width: usize) -> Vec<String> {
    render(q, width).iter().map(|l| strip(l)).collect()
}

#[test]
fn single_question_lines() {
    let q = Questionnaire::new(questions(vec![super::question(
        "Pick a db?",
        "DB",
        &["Postgres", "SQLite"],
        false,
    )]));
    let lines = plain(&q, 80);
    assert_eq!(lines[0], "[DB]");
    assert_eq!(lines[1], "Pick a db?");
    assert_eq!(lines[3], "❯ 1. Postgres");
    assert_eq!(lines[4], "     Postgres description");
    assert_eq!(lines[5], "  2. SQLite");
    assert_eq!(lines[7], "  3. Type something.");
    let hint = plain(&q, 100).last().cloned().unwrap();
    assert!(
        hint.starts_with("Enter to select · ↑/↓ to navigate · n to add notes"),
        "{hint}"
    );
    assert!(
        hint.ends_with("Ctrl+] to collapse · Esc to cancel"),
        "{hint}"
    );
    assert!(lines.last().unwrap().ends_with('…'));
}

#[test]
fn narrow_width_truncates_every_line() {
    let q = Questionnaire::new(two_questions());
    for line in render(&q, 24) {
        assert!(visible_width(&line) <= 24, "{line:?}");
    }
    let lines = plain(&q, 24);
    assert!(lines[0].starts_with("← "));
    assert!(lines[0].ends_with('…'));
}

#[test]
fn multi_select_rows_have_checkboxes_and_next() {
    let mut q = Questionnaire::new(two_questions());
    q.switch_tab(1);
    q.toggle(1);
    let lines = plain(&q, 80);
    assert!(lines.contains(&"❯ 1. [ ] Auth".to_owned()), "{lines:#?}");
    assert!(lines.contains(&"  2. [✔] Billing".to_owned()));
    assert!(lines.contains(&"  → Next".to_owned()));
}

#[test]
fn preview_side_by_side_when_wide_and_stacked_when_narrow() {
    let qs = questions(vec![json!({
        "question": "Layout?", "header": "L",
        "options": [
            { "label": "Grid", "description": "g", "preview": "GRID-PREVIEW" },
            { "label": "List", "description": "l" }
        ]
    })]);
    let q = Questionnaire::new(qs);
    let wide = plain(&q, 120);
    let row = wide
        .iter()
        .find(|l| l.contains("Grid") && l.contains('┌'))
        .cloned();
    assert!(row.is_some(), "{wide:#?}");
    assert!(
        wide.iter()
            .any(|l| l.contains("Grid") || l.contains("GRID-PREVIEW"))
    );
    assert!(
        wide.iter()
            .any(|l| l.starts_with("     g") && l.contains("│ GRID-PREVIEW"))
    );
    let narrow = plain(&q, 60);
    let options = narrow.iter().position(|l| l.contains("1. Grid")).unwrap();
    let preview = narrow
        .iter()
        .position(|l| l.contains("│ GRID-PREVIEW"))
        .unwrap();
    assert!(preview > options);
    assert!(narrow[preview].starts_with('│'));
}

#[test]
fn submit_tab_lists_missing_answers_and_collapsed_line() {
    let mut q = Questionnaire::new(two_questions());
    q.handle_raw("\r");
    q.switch_tab(2);
    let lines = plain(&q, 80);
    assert!(lines.contains(&"● DB: Postgres".to_owned()), "{lines:#?}");
    assert!(lines.contains(&"○ Features: not answered".to_owned()));
    assert!(lines.contains(&"⚠ Answer remaining questions before submitting:".to_owned()));
    assert!(lines.contains(&"❯ Submit answers".to_owned()));
    q.collapsed = true;
    assert_eq!(
        plain(&q, 80),
        ["? Submit · Ctrl+] to expand · Esc to cancel"]
    );
}
