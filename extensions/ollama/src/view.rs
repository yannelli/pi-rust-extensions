//! The `/ollama` panel state: list, details, delete confirmation, text input, and pull progress.

use pi_ext_sdk::Key;
use pi_ext_sdk::style::{self, truncate};

use crate::client::PullProgress;
use crate::discovery::{ModelInfo, human_size};

#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    Pull,
    Host,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    List,
    Details,
    ConfirmDelete,
    Input(Prompt, String),
    Pulling(String),
}

/// What the panel loop does after a key.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Close,
    Refresh,
    Delete(String),
    Pull(String),
    SetHost(String),
    CancelPull,
}

pub struct View {
    pub host: String,
    pub models: Vec<ModelInfo>,
    pub cursor: usize,
    pub mode: Mode,
    pub message: Option<String>,
    pub progress: Option<String>,
}

/// `status 45% (1.2 GB / 2.7 GB)` for a pull line.
pub fn progress_text(update: &PullProgress) -> String {
    match (update.completed, update.total) {
        (Some(done), Some(total)) if total > 0 => {
            let percent = done as f64 / total as f64 * 100.0;
            format!(
                "{} {percent:.0}% ({} / {})",
                update.status,
                human_size(done),
                human_size(total)
            )
        }
        _ => update.status.clone(),
    }
}

impl View {
    pub fn new(host: String, models: Vec<ModelInfo>) -> Self {
        Self {
            host,
            models,
            cursor: 0,
            mode: Mode::List,
            message: None,
            progress: None,
        }
    }

    pub fn set_models(&mut self, models: Vec<ModelInfo>) {
        self.models = models;
        self.cursor = self.cursor.min(self.models.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&ModelInfo> {
        self.models.get(self.cursor)
    }

    pub fn handle_key(&mut self, key: &Key) -> Action {
        match self.mode.clone() {
            Mode::Pulling(_) => {
                if key.is("escape") || key.is("ctrl+c") {
                    return Action::CancelPull;
                }
                Action::None
            }
            Mode::Input(prompt, mut text) => {
                match key {
                    Key::Escape | Key::Ctrl('c') => self.mode = Mode::List,
                    Key::Enter => {
                        self.mode = Mode::List;
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            return Action::None;
                        }
                        return match prompt {
                            Prompt::Pull => Action::Pull(text),
                            Prompt::Host => Action::SetHost(text),
                        };
                    }
                    Key::Backspace => {
                        text.pop();
                        self.mode = Mode::Input(prompt, text);
                    }
                    Key::Char(c) => {
                        text.push(*c);
                        self.mode = Mode::Input(prompt, text);
                    }
                    Key::Text(more) => {
                        text.push_str(more);
                        self.mode = Mode::Input(prompt, text);
                    }
                    _ => {}
                }
                Action::None
            }
            Mode::ConfirmDelete => {
                self.mode = Mode::List;
                match (key.is("y"), self.selected()) {
                    (true, Some(model)) => Action::Delete(model.tag.name.clone()),
                    _ => Action::None,
                }
            }
            Mode::Details => {
                if key.is("d") && self.selected().is_some() {
                    self.mode = Mode::ConfirmDelete;
                } else if key.is_cancel() || key.is("enter") {
                    self.mode = Mode::List;
                }
                Action::None
            }
            Mode::List => self.list_key(key),
        }
    }

    fn list_key(&mut self, key: &Key) -> Action {
        let count = self.models.len();
        match key {
            _ if key.is_cancel() => return Action::Close,
            Key::Up if count > 0 => self.cursor = self.cursor.checked_sub(1).unwrap_or(count - 1),
            Key::Down if count > 0 => self.cursor = (self.cursor + 1) % count,
            Key::Enter if count > 0 => self.mode = Mode::Details,
            _ if key.is("d") && count > 0 => self.mode = Mode::ConfirmDelete,
            _ if key.is("p") => self.mode = Mode::Input(Prompt::Pull, String::new()),
            _ if key.is("h") => self.mode = Mode::Input(Prompt::Host, self.host.clone()),
            _ if key.is("r") => return Action::Refresh,
            _ => {}
        }
        Action::None
    }

    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = vec![style::bold(&format!(
            "Ollama · {} · {} models",
            self.host,
            self.models.len()
        ))];
        match &self.mode {
            Mode::Details => {
                if let Some(model) = self.selected() {
                    lines.extend(model.details());
                }
                lines.push(style::dim("d delete · Esc back"));
            }
            Mode::ConfirmDelete => {
                let name = self
                    .selected()
                    .map(|m| m.tag.name.clone())
                    .unwrap_or_default();
                lines.push(format!("Delete {name}? y deletes, any other key keeps it."));
            }
            Mode::Input(prompt, text) => {
                let label = match prompt {
                    Prompt::Pull => "Model to pull (for example qwen3:8b)",
                    Prompt::Host => "Ollama host (saved to ~/.pi/agent/ext/ollama.json)",
                };
                lines.push(label.to_string());
                lines.push(format!("> {text}█"));
                lines.push(style::dim("Enter confirm · Esc cancel"));
            }
            Mode::Pulling(name) => {
                lines.push(format!("Pulling {name}"));
                lines.push(self.progress.clone().unwrap_or_else(|| "starting".into()));
                lines.push(style::dim("Esc cancels the pull"));
            }
            Mode::List => {
                if self.models.is_empty() {
                    lines.push(style::dim("No models. Press p to pull one."));
                }
                for (index, model) in self.models.iter().enumerate() {
                    let context = model
                        .context_length
                        .map_or("?".to_string(), |n| format!("{}k", n / 1000));
                    let row = format!(
                        "{:<28} {:>8} {:>6} ctx {:>5}  {}",
                        model.tag.name,
                        human_size(model.tag.size),
                        model.tag.details.parameter_size,
                        context,
                        model.capabilities.join(",")
                    );
                    lines.push(if index == self.cursor {
                        format!("→ {row}")
                    } else {
                        format!("  {row}")
                    });
                }
                lines.push(style::dim(
                    "↑↓ move · Enter details · p pull · d delete · r refresh · h host · q close",
                ));
            }
        }
        if let Some(message) = &self.message {
            lines.push(message.clone());
        }
        lines
            .into_iter()
            .map(|line| truncate(&line, width))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Tag;

    fn view() -> View {
        let model = |name: &str| ModelInfo {
            tag: Tag {
                name: name.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        View::new(
            "http://127.0.0.1:11434".into(),
            vec![model("a:1"), model("b:2")],
        )
    }

    #[test]
    fn keys_drive_the_list() {
        let mut view = view();
        assert_eq!(view.handle_key(&Key::Down), Action::None);
        assert_eq!(view.handle_key(&Key::Char('d')), Action::None);
        assert_eq!(view.mode, Mode::ConfirmDelete);
        assert_eq!(
            view.handle_key(&Key::Char('y')),
            Action::Delete("b:2".into())
        );
        view.handle_key(&Key::Char('p'));
        view.handle_key(&Key::Text("qwen3".into()));
        assert_eq!(view.handle_key(&Key::Enter), Action::Pull("qwen3".into()));
        view.handle_key(&Key::Enter);
        assert_eq!(view.mode, Mode::Details);
        assert!(view.render(80).iter().any(|line| line.contains("Model:")));
        view.handle_key(&Key::Escape);
        assert_eq!(view.handle_key(&Key::Char('r')), Action::Refresh);
        assert_eq!(view.handle_key(&Key::Char('q')), Action::Close);
    }

    #[test]
    fn progress_reads_percent_and_sizes() {
        let update = PullProgress {
            status: "pulling abc".into(),
            total: Some(2000),
            completed: Some(500),
            ..Default::default()
        };
        assert_eq!(progress_text(&update), "pulling abc 25% (500 B / 2.0 KB)");
        let plain = PullProgress {
            status: "verifying".into(),
            ..Default::default()
        };
        assert_eq!(progress_text(&plain), "verifying");
    }
}
