//! The `/statusline` editor as a key-driven state machine; `panel.rs` draws it in `ui.custom`.

use std::path::{Path, PathBuf};

use pi_ext_sdk::Key;
use pi_ext_sdk::style::{self, truncate};
use serde_json::{Value, json};

use crate::builtin::BuiltinStatusLine;
use crate::colors::ThemeColor;
use crate::config::{StatusLineSettings, status_line_choice};
use crate::editor_items::build_items;
use crate::layout::{change_layout, layout_items, layout_values};
use crate::list::{ListOutput, SettingsList};
use crate::segments::{Item, ItemKind, Segment};

pub type Save = Box<dyn FnMut(Value) -> Result<(), String> + Send>;

pub fn is_context(segment: Segment) -> bool {
    matches!(
        segment,
        Segment::Context
            | Segment::ContextUsed
            | Segment::ContextRemaining
            | Segment::ContextWindow
            | Segment::ContextBar
            | Segment::ContextTokens
            | Segment::ContextHeadroom
    )
}

pub fn edit_value(current: &Value, id: &str, selected: &str, home: &Path) -> Result<Value, String> {
    if id == "preset" {
        return status_line_choice(Some(current.clone()), selected);
    }
    if id == "source" && selected != "builtin" {
        crate::external::validate_source(selected, home)?;
        return status_line_choice(Some(current.clone()), selected);
    }
    let mut value = current.clone();
    value["type"] = json!("builtin");
    match id {
        "source" => {}
        "segments" => {
            value[id] = serde_json::from_str(selected).map_err(|error| error.to_string())?
        }
        "padding" => value[id] = json!(selected.parse::<u8>().map_err(|error| error.to_string())?),
        "context" => {
            let settings: StatusLineSettings =
                serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
            let mut lines = layout_values(&BuiltinStatusLine::from_settings(&settings, 0));
            let mut replaced = false;
            for line in &mut lines {
                line.retain_mut(|item| {
                    let context = Item::parse(item)
                        .is_some_and(|item| matches!(item.kind, ItemKind::Segment(segment) if is_context(segment)));
                    if !context {
                        return true;
                    }
                    if selected == "hidden" || replaced {
                        return false;
                    }
                    item["segment"] = json!(selected);
                    replaced = true;
                    true
                });
            }
            if selected != "hidden" && !replaced {
                if lines.is_empty() {
                    lines.push(Vec::new());
                }
                lines[0].push(json!(selected));
            }
            value["segments"] = json!(lines);
        }
        _ if selected == "preset" => {
            let object = value.as_object_mut().expect("settings object");
            object.remove(id);
            if id == "chip" {
                object.remove("style");
            }
        }
        _ => value[id] = json!(selected),
    }
    Ok(value)
}

pub struct StatusLineEditor {
    pub value: Value,
    list: SettingsList,
    layout: Option<(Vec<Vec<Value>>, SettingsList)>,
    save: Save,
    home: PathBuf,
    config_path: PathBuf,
    pub error: Option<String>,
    pub closed: bool,
}

impl StatusLineEditor {
    pub fn new(value: Option<Value>, home: PathBuf, save: Save) -> Self {
        let value = value
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({"type": "off"}));
        let list = SettingsList::new(build_items(&value));
        Self {
            value,
            list,
            layout: None,
            save,
            home,
            config_path: PathBuf::from("~/.pi/agent/ext/statusline.json"),
            error: None,
            closed: false,
        }
    }

    pub fn select_item(&mut self, id: &str) {
        self.list.select_item(id);
    }

    pub fn with_config_path(mut self, path: PathBuf) -> Self {
        self.config_path = path;
        self
    }

    pub fn handle_input(&mut self, data: &str) {
        self.handle_key(&Key::parse(data));
    }

    pub fn handle_key(&mut self, key: &Key) {
        if let Some((mut lines, mut list)) = self.layout.take() {
            match list.handle_key(key) {
                ListOutput::Cancel => return,
                ListOutput::Change(id, _) if id == "apply" => {
                    return self.apply_change("segments", &json!(lines).to_string());
                }
                ListOutput::Change(id, selected) => {
                    change_layout(&mut lines, &id, &selected);
                    list.replace(layout_items(&lines), &id);
                }
                ListOutput::None | ListOutput::Open(_) => {}
            }
            self.layout = Some((lines, list));
            return;
        }
        match self.list.handle_key(key) {
            ListOutput::Cancel => self.closed = true,
            ListOutput::Change(id, selected) => self.apply_change(&id, &selected),
            ListOutput::Open(_) => {
                let settings: StatusLineSettings =
                    serde_json::from_value(self.value.clone()).unwrap_or_default();
                let mut lines = layout_values(&BuiltinStatusLine::from_settings(&settings, 0));
                if lines.is_empty() {
                    lines.push(Vec::new());
                }
                let list = SettingsList::new(layout_items(&lines));
                self.layout = Some((lines, list));
            }
            ListOutput::None => {}
        }
    }

    pub fn apply_change(&mut self, id: &str, selected: &str) {
        let result = edit_value(&self.value, id, selected, &self.home).and_then(|value| {
            (self.save)(value.clone())?;
            Ok(value)
        });
        match result {
            Ok(value) => {
                self.value = value;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        self.list.replace(build_items(&self.value), id);
    }

    pub fn render(&self, width: usize, preview: &[String]) -> Vec<String> {
        let mut lines = vec![
            ThemeColor::Accent.paint(&format!("Status line · {}", self.config_path.display())),
        ];
        if let Some(error) = &self.error {
            lines.push(ThemeColor::Error.paint(error));
        }
        lines.extend(preview.iter().cloned());
        lines.push(style::dim(&"─".repeat(width.min(60))));
        match &self.layout {
            Some((_, list)) => lines.extend(list.render(width)),
            None => lines.extend(self.list.render(width)),
        }
        lines
            .into_iter()
            .map(|line| truncate(&line, width))
            .collect()
    }
}
