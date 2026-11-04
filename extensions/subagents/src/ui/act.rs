//! Saving the form, the prompt and raw file edits, and the runs tab.

use std::path::{Path, PathBuf};

use pi_ext_sdk::Key;

use crate::runs::Run;
use crate::runs_panel::{Action as RunsAction, RunsView};
use crate::ui::draft::{FIELDS, Field};
use crate::ui::flows::{After, Modal, Pending};
use crate::ui::ops::{self, Entry};
use crate::ui::state::{Effect, Focus, Level, State, Tab};

impl State {
    /// Saves the form; asks for a scope when the agent has no file. `true` when it saved.
    pub fn save(&mut self, after: Option<After>) -> bool {
        let Some(draft) = self.draft.clone() else {
            return false;
        };
        let problems = draft.problems(&self.known_tools());
        if let Some(problem) = problems.into_iter().find(|p| p.error) {
            self.err(format!("Not saved: {}", problem.text));
            return false;
        }
        if !draft.dirty() {
            self.message = Some((Level::Info, "No changes to save.".into()));
            return true;
        }
        let top = self.selected().and_then(Entry::top).and_then(Path::parent);
        match (top.map(PathBuf::from), &self.setup.layers.project) {
            (Some(dir), _) => self.save_to(&dir),
            (None, Some(_)) => {
                let title = format!("Save the {} override to", draft.name);
                let pending = Pending::Save(after);
                self.modal = Some(Modal::scope(&self.setup.layers, &title, pending));
                false
            }
            (None, None) => {
                let dir = self.setup.layers.global.clone();
                self.save_to(&dir)
            }
        }
    }

    pub fn save_to(&mut self, dir: &Path) -> bool {
        let Some(draft) = self.draft.clone() else {
            return false;
        };
        let plugin = self.selected().and_then(|e| e.plugin.clone());
        let changed = draft.changed();
        let listed = changed.contains(&Field::Description) || changed.contains(&Field::Exposure);
        let saved = self.done(ops::save(dir, &draft), listed);
        if saved && let Some(plugin) = plugin {
            let text = format!(
                "Saved. The {plugin} plugin owns this file and an update replaces it; copy it (c) to keep changes."
            );
            self.message = Some((Level::Info, text));
        }
        saved
    }

    pub fn edit_prompt(&mut self) -> Effect {
        let Some(draft) = &self.draft else {
            return Effect::None;
        };
        let title = format!("System prompt of {}", draft.name);
        let text = draft.values.prompt.clone();
        self.focus = Focus::Form;
        self.field = FIELDS.iter().position(|f| *f == Field::Prompt).unwrap_or(0);
        Effect::EditPrompt { title, text }
    }

    pub fn prompt_edited(&mut self, text: Option<String>) {
        if let (Some(text), Some(draft)) = (text, self.draft.as_mut()) {
            draft.values.prompt = text.trim().to_owned();
        }
    }

    pub fn edit_file(&mut self) -> Effect {
        let Some(path) = self.selected().and_then(Entry::top).map(PathBuf::from) else {
            self.err("No file yet: change a field and save (s) to create one.");
            return Effect::None;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Effect::EditFile { path, text },
            Err(err) => {
                self.err(format!("Cannot read {}: {err}", path.display()));
                Effect::None
            }
        }
    }

    pub fn file_edited(&mut self, path: &Path, text: Option<String>) {
        let Some(text) = text else {
            return;
        };
        let name = self.selected().map(|e| e.def.name.clone());
        self.done(ops::write_raw(path, &name.unwrap_or_default(), &text), true);
    }

    pub fn open_runs(&mut self, agent: Option<String>) {
        self.tab = Tab::Runs;
        self.focus = Focus::List;
        self.runs = RunsView::default();
        self.runs_agent = agent;
    }

    /// The runs the runs tab shows: all, or those of one agent.
    pub fn shown_runs(&self, runs: &[Run]) -> Vec<Run> {
        let mine = |r: &&Run| self.runs_agent.as_ref().is_none_or(|a| &r.agent == a);
        runs.iter().filter(mine).cloned().collect()
    }

    pub fn runs_key(&mut self, key: &Key, runs: &[Run]) -> Effect {
        let shown = self.shown_runs(runs);
        if self.runs.detail.is_none() {
            match key {
                Key::Tab | Key::BackTab => {
                    self.tab = Tab::Agents;
                    return Effect::None;
                }
                Key::Char('a') if self.runs_agent.is_some() => {
                    self.open_runs(None);
                    return Effect::None;
                }
                Key::Escape if self.runs_agent.is_some() => {
                    self.tab = Tab::Agents;
                    return Effect::None;
                }
                _ => {}
            }
        }
        match self.runs.key(key, &shown) {
            RunsAction::Close => Effect::Close,
            RunsAction::Cancel(id) => Effect::CancelRun(id),
            RunsAction::Redraw => Effect::None,
        }
    }
}
