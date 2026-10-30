//! Answer transitions: confirm, multi-select, notes, edits, done.

use crate::result::{AnswerKind, QuestionAnswer, QuestionnaireResult};
use crate::state::{EditTarget, Effect, Questionnaire, Row};

impl Questionnaire {
    pub fn done(&self, cancelled: bool) -> Effect {
        Effect::Done(QuestionnaireResult {
            answers: self.answers.values().cloned().collect(),
            cancelled,
            global_note: self.pending_note(self.questions.len()),
            ..QuestionnaireResult::default()
        })
    }

    fn finish_or_advance(&mut self) -> Option<Effect> {
        if !self.is_multi() {
            return Some(self.done(false));
        }
        let next = (self.current_tab + 1).min(self.questions.len());
        self.switch_tab(next);
        None
    }

    /// The single-select answer for `row`, or for the input draft in input mode.
    pub fn single_answer(&self, row: Option<Row>) -> Option<QuestionAnswer> {
        let q = self.question()?;
        if self.input_mode {
            if self.input.trim().is_empty() {
                return None;
            }
            let answer = Some(self.input.clone());
            return Some(QuestionAnswer::new(
                self.current_tab,
                &q.question,
                AnswerKind::Custom,
                answer,
            ));
        }
        match row? {
            Row::Option(i) => Some(QuestionAnswer::new(
                self.current_tab,
                &q.question,
                AnswerKind::Option,
                Some(q.options[i].label.clone()),
            )),
            Row::Other | Row::Next => None,
        }
    }

    pub fn confirm(&mut self, mut answer: QuestionAnswer) -> Option<Effect> {
        let index = answer.question_index;
        if answer.kind == AnswerKind::Option {
            let label = answer.answer.clone().unwrap_or_default();
            let option = self.questions[index]
                .options
                .iter()
                .find(|o| o.label == label);
            answer.preview = option.and_then(|o| o.preview().map(str::to_owned));
        }
        if let Some(note) = self.pending_note(index) {
            answer.notes = Some(note);
        }
        if answer.kind == AnswerKind::Custom {
            self.drafts.remove(&index);
        }
        self.answers.insert(index, answer);
        self.finish_or_advance()
    }

    pub fn multi_confirm(&mut self) -> Option<Effect> {
        let tab = self.current_tab;
        let question = self.question()?.question.clone();
        let custom = self.input.clone();
        let custom = (!custom.trim().is_empty()).then_some(custom);
        let mut answer = QuestionAnswer::new(tab, &question, AnswerKind::Multi, custom);
        answer.selected = Some(self.selected_labels());
        answer.notes = self.pending_note(tab);
        self.answers.insert(tab, answer);
        self.drafts.remove(&tab);
        self.sync_checked();
        self.finish_or_advance()
    }

    pub fn toggle(&mut self, index: usize) {
        if !self.checked.remove(&index) {
            self.checked.insert(index);
        }
        self.persist_multi();
    }

    pub fn notes_enter(&mut self) {
        self.notes_draft = self.note_for(self.current_tab);
        self.notes_visible = true;
    }

    pub fn notes_exit(&mut self) {
        let tab = self.current_tab;
        let trimmed = self.notes_draft.trim().to_owned();
        if trimmed.is_empty() {
            self.notes.remove(&tab);
        } else {
            self.notes.insert(tab, trimmed.clone());
        }
        if let Some(answer) = self.answers.get_mut(&tab) {
            answer.notes = (!trimmed.is_empty()).then_some(trimmed);
        }
        self.notes_visible = false;
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.drafts.insert(self.current_tab, String::new());
    }

    /// Text from the external editor; `None` keeps the draft.
    pub fn apply_edit(&mut self, target: EditTarget, text: Option<String>) {
        let Some(text) = text else { return };
        let text = text.trim_end_matches('\n').to_owned();
        match target {
            EditTarget::Input => {
                self.drafts.insert(self.current_tab, text.clone());
                self.input = text;
            }
            EditTarget::Notes => self.notes_draft = text,
        }
    }
}
