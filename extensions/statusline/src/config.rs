//! The `statusLine` value: `builtin` draws segments natively; `command` runs a shell command
//! with session JSON on stdin; `claude`, `cursor`, and `codex` reuse those agents' status lines.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::builtin::BuiltinStatusLine;
use crate::external;
use crate::presets::preset_names;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusLineSettings {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_interval: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// The chip shape; `style` is its older name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// Segment names for one line, or a list of lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    /// Segment name to a theme color token or `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommandConfig {
    pub command: String,
    pub padding: usize,
    /// Also rerun on this interval (`refreshInterval`, at least one second).
    pub refresh: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatusLineConfig {
    Builtin(BuiltinStatusLine),
    Command(CommandConfig),
}

impl StatusLineConfig {
    /// `None` when the type is `off` or unknown, or no command is found.
    pub fn from_settings(settings: &StatusLineSettings, home: &Path) -> Option<Self> {
        let padding = |value: Option<f64>| {
            value
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(0.0)
                .min(40.0) as usize
        };
        let refresh = |value: Option<f64>| {
            value
                .filter(|seconds| seconds.is_finite() && *seconds >= 1.0)
                .map(Duration::from_secs_f64)
        };
        let command = |command: String, padding: usize, refresh: Option<Duration>| {
            Self::Command(CommandConfig {
                command,
                padding,
                refresh,
            })
        };
        let pad = padding(settings.padding);
        match settings.kind.as_deref() {
            Some("builtin") | None => Some(Self::Builtin(BuiltinStatusLine::from_settings(
                settings, pad,
            ))),
            Some("command") => {
                let text = settings
                    .command
                    .clone()
                    .filter(|command| !command.trim().is_empty())?;
                Some(command(text, pad, refresh(settings.refresh_interval)))
            }
            Some(kind @ ("claude" | "cursor")) => {
                let path = if kind == "claude" {
                    external::claude_settings_path(home)
                } else {
                    external::cursor_config_path(home)
                };
                let found = external::read_command(&path)?;
                let pad = settings.padding.map_or(padding(found.padding), |_| pad);
                Some(command(
                    found.command,
                    pad,
                    refresh(settings.refresh_interval.or(found.refresh_interval)),
                ))
            }
            Some("codex") => {
                let items = external::read_codex_items(&external::codex_config_path(home))
                    .unwrap_or_default();
                Some(Self::Builtin(BuiltinStatusLine::from_codex(&items, pad)))
            }
            Some(_) => None,
        }
    }
}

/// The `statusLine` value after `/statusline <choice>`. A preset clears `chip`, `style`,
/// `theme`, `icons`, `segments`, and `separator`; `command`, `colors`, and `padding` stay.
pub fn status_line_choice(current: Option<Value>, choice: &str) -> Result<Value, String> {
    let mut object = match current {
        Some(Value::Object(object)) => object,
        _ => serde_json::Map::new(),
    };
    if preset_names().any(|preset| preset == choice) {
        for key in [
            "chip",
            "style",
            "theme",
            "icons",
            "segments",
            "separator",
            "layout",
        ] {
            object.remove(key);
        }
        object.insert("type".into(), json!("builtin"));
        object.insert("preset".into(), json!(choice));
        return Ok(Value::Object(object));
    }
    match choice {
        "command" => {
            if !object
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| !command.trim().is_empty())
            {
                return Err("Set \"command\" in ~/.pi/agent/ext/statusline.json first.".into());
            }
            object.insert("type".into(), json!("command"));
        }
        "from-claude" | "from-cursor" | "from-codex" => {
            object.insert("type".into(), json!(&choice["from-".len()..]));
        }
        "off" => {
            object.insert("type".into(), json!("off"));
        }
        _ => {
            return Err(format!(
                "Unknown status line \"{choice}\". Use {}.",
                status_line_choices()
            ));
        }
    }
    Ok(Value::Object(object))
}

pub fn status_line_choices() -> String {
    let presets: Vec<&str> = preset_names().collect();
    format!(
        "{}, command, from-claude, from-cursor, from-codex, or off",
        presets.join(", ")
    )
}

/// A short name for the active `statusLine`, such as `pills` or `from-claude`.
pub fn status_line_name(settings: Option<&StatusLineSettings>) -> String {
    match settings.map(|settings| (settings.kind.as_deref(), settings.preset.as_deref())) {
        None => "default footer".into(),
        Some((Some("builtin") | None, preset)) => preset.unwrap_or("minimal").into(),
        Some((Some("command"), _)) => "command".into(),
        Some((Some(kind @ ("claude" | "cursor" | "codex")), _)) => format!("from-{kind}"),
        Some((Some(_), _)) => "off".into(),
    }
}
