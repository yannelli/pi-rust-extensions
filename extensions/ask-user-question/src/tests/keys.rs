use super::{question, questions, two_questions};
use crate::result::AnswerKind;
use crate::state::{EditTarget, Effect, Questionnaire, Row};

const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const TAB: &str = "\t";
const ENTER: &str = "\r";
const ESC: &str = "\x1b";
const SHIFT_ENTER: &str = "\x1b\r";

fn press(q: &mut Questionnaire, keys: &[&str]) -> Option<Effect> {
    let mut last = None;
    for key in keys {
        last = q.handle_raw(key);
    }
    last
}

fn done(effect: Option<Effect>) -> crate::result::QuestionnaireResult {
    match effect {
        Some(Effect::Done(result)) => result,
        other => panic!("expected done, got {other:?}"),
    }
}

fn single() -> Questionnaire {
    Questionnaire::new(questions(vec![question("Pick?", "P", &["A", "B"], false)]))
}

#[test]
fn single_select_enter_answers_and_finishes() {
    let mut q = single();
    let result = done(press(&mut q, &[DOWN, ENTER]));
    assert!(!result.cancelled);
    assert_eq!(result.answers[0].answer.as_deref(), Some("B"));
    assert_eq!(result.answers[0].kind, AnswerKind::Option);
}

#[test]
fn navigation_wraps_and_digits_answer() {
    let mut q = single();
    press(&mut q, &[UP]);
    assert_eq!(q.current_row(), Some(Row::Other));
    assert!(q.input_mode);
    press(&mut q, &[DOWN]);
    assert_eq!(q.option_index, 0);
    assert!(!q.input_mode);
    let result = done(press(&mut q, &["2"]));
    assert_eq!(result.answers[0].answer.as_deref(), Some("B"));
}

#[test]
fn free_text_editing() {
    let mut q = single();
    press(&mut q, &["3"]);
    assert!(q.input_mode);
    assert!(
        press(&mut q, &[ENTER]).is_none(),
        "blank draft is not an answer"
    );
    press(&mut q, &["h", "i", "x", "\x7f", SHIFT_ENTER, "y"]);
    assert_eq!(q.input, "hi\ny");
    press(&mut q, &["\x15"]);
    assert_eq!(q.input, "");
    press(&mut q, &["o", "k"]);
    assert_eq!(
        press(&mut q, &["\x07"]),
        Some(Effect::Edit(EditTarget::Input, "ok".into()))
    );
    q.apply_edit(EditTarget::Input, Some("edited\n".into()));
    let result = done(press(&mut q, &[ENTER]));
    assert_eq!(result.answers[0].kind, AnswerKind::Custom);
    assert_eq!(result.answers[0].answer.as_deref(), Some("edited"));
}

#[test]
fn multi_select_toggles_and_next_commits() {
    let mut q = Questionnaire::new(questions(vec![question(
        "Many?",
        "M",
        &["A", "B", "C"],
        true,
    )]));
    press(&mut q, &[" ", DOWN, ENTER, "3", "3"]);
    assert_eq!(q.selected_labels(), ["A", "B"]);
    press(&mut q, &["4"]);
    assert!(q.input_mode);
    press(&mut q, &["x", DOWN]);
    assert_eq!(q.current_row(), Some(Row::Next));
    let result = done(press(&mut q, &[" ", ENTER]));
    let answer = &result.answers[0];
    assert_eq!(answer.kind, AnswerKind::Multi);
    assert_eq!(
        answer.selected.as_deref(),
        Some(&["A".to_owned(), "B".to_owned()][..])
    );
    assert_eq!(answer.answer.as_deref(), Some("x"));
}

#[test]
fn notes_attach_to_answers() {
    let mut q = single();
    press(&mut q, &["n", "n", "o", "t", "e", ESC]);
    assert!(!q.notes_visible);
    assert_eq!(q.note_for(0), "note");
    assert!(q.answers.is_empty(), "notes do not answer a question");
    let result = done(press(&mut q, &[ENTER]));
    assert_eq!(result.answers[0].notes.as_deref(), Some("note"));
}

#[test]
fn tabs_submit_and_global_note() {
    let mut q = Questionnaire::new(two_questions());
    assert_eq!(q.total_tabs(), 3);
    assert!(press(&mut q, &[ENTER]).is_none());
    assert_eq!(q.current_tab, 1, "answering advances to the next tab");
    press(&mut q, &[TAB]);
    assert!(q.on_submit_tab());
    press(&mut q, &["n", "g", ENTER]);
    let result = done(press(&mut q, &[ENTER]));
    assert!(!result.cancelled);
    assert_eq!(result.answers.len(), 1);
    assert_eq!(result.global_note.as_deref(), Some("g"));
    let mut q = Questionnaire::new(two_questions());
    press(&mut q, &["\x1b[Z"]);
    assert!(q.on_submit_tab());
    assert!(done(press(&mut q, &[DOWN, ENTER])).cancelled);
}

#[test]
fn escape_cancels_and_collapse_locks_keys() {
    assert!(done(press(&mut single(), &[ESC])).cancelled);
    let mut q = single();
    press(&mut q, &["\x1d"]);
    assert!(q.collapsed);
    assert!(press(&mut q, &[ENTER, DOWN]).is_none());
    assert_eq!(q.option_index, 0);
    press(&mut q, &["\x1d"]);
    assert!(!q.collapsed);
    let mut q = single();
    press(&mut q, &["\x1d"]);
    assert!(done(press(&mut q, &[ESC])).cancelled);
}

#[test]
fn custom_keys_from_config() {
    let mut q = single().with_keys("alt+o", "т");
    press(&mut q, &["\x1bo"]);
    assert!(q.collapsed);
    press(&mut q, &["\x1bo", "n"]);
    assert!(!q.notes_visible);
    press(&mut q, &["т"]);
    assert!(q.notes_visible);
}
