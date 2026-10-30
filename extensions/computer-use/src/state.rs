use std::collections::{BTreeMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::outline::{Outline, Rect};

pub const STATE_CAP: usize = 128;
pub const OUTPUT_CAP: usize = 64;
pub const TOOL_NAMES: [&str; 11] = [
    "find_roots",
    "observe_ui",
    "search_ui",
    "expand_ui",
    "inspect_ui",
    "act_ui",
    "read_text",
    "wait_for",
    "launch_browser",
    "navigate_browser",
    "evaluate_browser",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Root {
    pub window_ref: String,
    pub app: String,
    pub bundle_id: Option<String>,
    pub pid: i64,
    pub window_title: String,
    pub window_id: i64,
    pub frame_points: Rect,
    pub scale_factor: f64,
    pub is_minimized: bool,
    pub is_onscreen: bool,
    pub is_focused: bool,
    pub kind: Option<String>,
    pub page_id: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Target {
    pub app: String,
    pub bundle_id: Option<String>,
    pub pid: i64,
    pub window_title: String,
    pub window_id: i64,
    pub window_ref: Option<String>,
    pub frame_points: Rect,
    pub page_id: Option<String>,
    pub cdp_port: Option<u16>,
}

impl Target {
    pub fn from_root(root: &Root, cdp_port: Option<u16>) -> Self {
        Self {
            app: root.app.clone(),
            bundle_id: root.bundle_id.clone(),
            pid: root.pid,
            window_title: root.window_title.clone(),
            window_id: root.window_id,
            window_ref: Some(root.window_ref.clone()).filter(|r| !r.is_empty()),
            frame_points: root.frame_points,
            page_id: root.page_id.clone(),
            cdp_port: root.page_id.as_ref().and(cdp_port),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Capture {
    pub state_id: String,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub timestamp: u64,
    pub coordinate_space: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub target: Target,
    pub capture: Capture,
    pub outline: Outline,
}

#[derive(Debug, Default)]
pub struct Store {
    pub roots: BTreeMap<String, Root>,
    next_root: usize,
    states: VecDeque<Observation>,
    outputs: VecDeque<(String, String)>,
    next_output: usize,
    counter: u64,
}

pub fn now_ms() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis() as u64
}

impl Store {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn new_state_id(&mut self) -> String {
        self.counter += 1;
        format!("s{:x}-{}-{}", now_ms(), std::process::id(), self.counter)
    }

    /// Reuses the `@r` ref of a window already seen with the same pid and id.
    pub fn put_root(&mut self, mut root: Root) -> Root {
        let same = |r: &&Root| {
            r.pid == root.pid && r.window_id == root.window_id && r.page_id == root.page_id
        };
        let known = self.roots.values().find(same);
        root.window_ref = match known {
            Some(existing) => existing.window_ref.clone(),
            None => {
                self.next_root += 1;
                format!("@r{}", self.next_root)
            }
        };
        self.roots.insert(root.window_ref.clone(), root.clone());
        root
    }

    pub fn put_state(&mut self, observation: Observation) {
        let id = &observation.capture.state_id;
        self.states.retain(|o| &o.capture.state_id != id);
        if self.states.len() >= STATE_CAP {
            self.states.pop_front();
        }
        self.states.push_back(observation);
    }

    pub fn state(&self, state_id: &str) -> Option<&Observation> {
        self.states.iter().find(|o| o.capture.state_id == state_id)
    }

    pub fn latest_for(&self, target: &Target) -> Option<&Observation> {
        let same = |o: &&Observation| {
            o.target.pid == target.pid
                && o.target.window_id == target.window_id
                && o.target.page_id == target.page_id
        };
        self.states.iter().rev().find(same)
    }

    pub fn put_output(&mut self, text: String) -> String {
        self.next_output += 1;
        let reference = format!("@o{}", self.next_output);
        if self.outputs.len() >= OUTPUT_CAP {
            self.outputs.pop_front();
        }
        self.outputs.push_back((reference.clone(), text));
        reference
    }

    pub fn output(&self, reference: &str) -> Option<&str> {
        let entry = self.outputs.iter().find(|(r, _)| r == reference);
        entry.map(|(_, text)| text.as_str())
    }

    fn restore_roots(&mut self, windows: &[Value]) {
        for window in windows {
            let Ok(root) = serde_json::from_value::<Root>(window.clone()) else {
                continue;
            };
            let Some(index) = root
                .window_ref
                .strip_prefix("@r")
                .and_then(|n| n.parse().ok())
            else {
                continue;
            };
            self.next_root = self.next_root.max(index);
            self.roots.entry(root.window_ref.clone()).or_insert(root);
        }
    }

    /// Rebuilds roots and states from tool result `details` on the branch, newest first.
    pub fn restore_from_branch(&mut self, entries: &[Value]) {
        self.clear();
        let mut restored = Vec::new();
        for entry in entries.iter().rev() {
            if entry["type"] != "message" || entry["message"]["role"] != "toolResult" {
                continue;
            }
            let message = &entry["message"];
            if !message["toolName"]
                .as_str()
                .is_some_and(|n| TOOL_NAMES.contains(&n))
            {
                continue;
            }
            let details = &message["details"];
            if let Some(windows) = details["windows"].as_array() {
                self.restore_roots(windows);
                continue;
            }
            let Some(observation) = observation_from_details(details) else {
                continue;
            };
            let target = &observation.target;
            let key = (target.pid, target.window_id, target.page_id.clone());
            if !restored.contains(&key) {
                restored.push(key);
                self.states.push_front(observation);
            }
        }
        while self.states.len() > STATE_CAP {
            self.states.pop_front();
        }
    }
}

pub fn observation_from_details(details: &Value) -> Option<Observation> {
    let target: Target = serde_json::from_value(details.get("target")?.clone()).ok()?;
    let capture: Capture = serde_json::from_value(details.get("capture")?.clone()).ok()?;
    let outline: Outline = serde_json::from_value(details.get("outline")?.clone()).ok()?;
    if target.app.is_empty() || capture.state_id.is_empty() || outline.root.reference.is_empty() {
        return None;
    }
    Some(Observation {
        target,
        capture,
        outline,
    })
}
