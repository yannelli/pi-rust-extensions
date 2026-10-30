//! Questionnaire state; `answer.rs` holds the answer transitions and `keys.rs` maps keys.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::config::{DEFAULT_COLLAPSE_KEY, DEFAULT_NOTES_KEY};
use crate::result::{AnswerKind, QuestionAnswer, QuestionnaireResult};
use crate::schema::Question;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Option(usize),
    Other,
    Next,
}

impl Row {
    pub fn numbered(self) -> bool {
        !matches!(self, Row::Next)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    Input,
    Notes,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Done(QuestionnaireResult),
    /// Open the external editor on the draft; feed the text back with `apply_edit`.
    Edit(EditTarget, String),
}

#[derive(Debug, Clone)]
pub struct Questionnaire {
    pub questions: Vec<Question>,
    pub rows: Vec<Vec<Row>>,
    pub current_tab: usize,
    pub option_index: usize,
    pub input_mode: bool,
    pub notes_visible: bool,
    pub collapsed: bool,
    pub answers: BTreeMap<usize, QuestionAnswer>,
    pub checked: BTreeSet<usize>,
    pub drafts: HashMap<usize, String>,
    pub input: String,
    pub notes: HashMap<usize, String>,
    pub notes_draft: String,
    pub submit_choice: usize,
    pub collapse_key: String,
    pub notes_key: String,
}

pub fn rows_for(question: &Question) -> Vec<Row> {
    let mut rows: Vec<Row> = (0..question.options.len()).map(Row::Option).collect();
    rows.push(Row::Other);
    if question.is_multi() {
        rows.push(Row::Next);
    }
    rows
}

impl Questionnaire {
    pub fn new(questions: Vec<Question>) -> Self {
        let rows = questions.iter().map(rows_for).collect();
        Self {
            questions,
            rows,
            current_tab: 0,
            option_index: 0,
            input_mode: false,
            notes_visible: false,
            collapsed: false,
            answers: BTreeMap::new(),
            checked: BTreeSet::new(),
            drafts: HashMap::new(),
            input: String::new(),
            notes: HashMap::new(),
            notes_draft: String::new(),
            submit_choice: 0,
            collapse_key: DEFAULT_COLLAPSE_KEY.into(),
            notes_key: DEFAULT_NOTES_KEY.into(),
        }
    }

    pub fn with_keys(mut self, collapse_key: &str, notes_key: &str) -> Self {
        self.collapse_key = collapse_key.into();
        self.notes_key = notes_key.into();
        self
    }

    /// More than one question: tabs plus a Submit tab.
    pub fn is_multi(&self) -> bool {
        self.questions.len() > 1
    }

    pub fn total_tabs(&self) -> usize {
        if self.is_multi() {
            self.questions.len() + 1
        } else {
            1
        }
    }

    pub fn on_submit_tab(&self) -> bool {
        self.is_multi() && self.current_tab == self.questions.len()
    }

    pub fn question(&self) -> Option<&Question> {
        self.questions.get(self.current_tab)
    }

    pub fn current_rows(&self) -> &[Row] {
        self.rows.get(self.current_tab).map_or(&[], Vec::as_slice)
    }

    pub fn current_row(&self) -> Option<Row> {
        self.current_rows().get(self.option_index).copied()
    }

    pub fn custom_draft(&self, tab: usize) -> String {
        if let Some(draft) = self.drafts.get(&tab) {
            return draft.clone();
        }
        match self.answers.get(&tab) {
            Some(a) if matches!(a.kind, AnswerKind::Custom | AnswerKind::Multi) => {
                a.answer.clone().unwrap_or_default()
            }
            _ => String::new(),
        }
    }

    pub fn note_for(&self, tab: usize) -> String {
        self.notes
            .get(&tab)
            .cloned()
            .or_else(|| self.answers.get(&tab).and_then(|a| a.notes.clone()))
            .unwrap_or_default()
    }

    pub(crate) fn pending_note(&self, tab: usize) -> Option<String> {
        self.notes.get(&tab).filter(|n| !n.is_empty()).cloned()
    }

    pub fn selected_labels(&self) -> Vec<String> {
        let Some(q) = self.question() else {
            return Vec::new();
        };
        (0..q.options.len())
            .filter(|i| self.checked.contains(i))
            .map(|i| q.options[i].label.clone())
            .collect()
    }

    pub(crate) fn sync_checked(&mut self) {
        self.checked.clear();
        let Some(q) = self
            .questions
            .get(self.current_tab)
            .filter(|q| q.is_multi())
        else {
            return;
        };
        let labels = self
            .answers
            .get(&self.current_tab)
            .and_then(|a| a.selected.clone())
            .unwrap_or_default();
        for (i, option) in q.options.iter().enumerate() {
            if labels.contains(&option.label) {
                self.checked.insert(i);
            }
        }
    }

    pub fn persist_multi(&mut self) {
        let tab = self.current_tab;
        let Some(q) = self.questions.get(tab).filter(|q| q.is_multi()) else {
            return;
        };
        let question = q.question.clone();
        let selected = self.selected_labels();
        let custom = self.custom_draft(tab);
        let has_custom = !custom.trim().is_empty();
        if selected.is_empty() && !has_custom {
            self.answers.remove(&tab);
            return;
        }
        let mut answer = QuestionAnswer::new(
            tab,
            &question,
            AnswerKind::Multi,
            has_custom.then_some(custom),
        );
        answer.selected = Some(selected);
        answer.notes = self.pending_note(tab);
        self.answers.insert(tab, answer);
    }

    pub fn switch_tab(&mut self, next: usize) {
        let rows = self.rows.get(next).map_or(&[][..], Vec::as_slice);
        let index = match self.answers.get(&next) {
            Some(a) if a.kind == AnswerKind::Option => rows.iter().position(|row| match row {
                Row::Option(i) => {
                    a.answer.as_ref() == Some(&self.questions[next].options[*i].label)
                }
                _ => false,
            }),
            Some(a) if a.kind == AnswerKind::Multi => rows.iter().position(|r| *r == Row::Next),
            _ => None,
        };
        self.current_tab = next;
        self.option_index = index.unwrap_or(0);
        self.input_mode = false;
        self.notes_visible = false;
        self.submit_choice = 0;
        self.sync_checked();
        self.notes_draft = self.note_for(next);
        self.input = self.custom_draft(next);
    }

    pub fn nav(&mut self, next: usize) {
        let entering_input = self.current_rows().get(next).copied() == Some(Row::Other);
        if self.input_mode {
            self.drafts.insert(self.current_tab, self.input.clone());
        }
        let leaving_input = self.input_mode && !entering_input;
        self.option_index = next;
        self.input_mode = entering_input;
        if leaving_input && self.question().is_some_and(Question::is_multi) {
            self.persist_multi();
        }
        if entering_input {
            self.input = self.custom_draft(self.current_tab);
        }
    }
}
