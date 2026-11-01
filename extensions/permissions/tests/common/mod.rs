use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use pi_ext_permissions::{Paths, extension};
use pi_ext_sdk::test_util::TestHost;
use serde_json::{Value, json};

pub struct Fx {
    pub _dir: tempfile::TempDir,
    pub paths: Paths,
    pub agent: PathBuf,
    pub cwd: PathBuf,
}

pub fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cwd = root.join("project");
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    let paths = Paths {
        home: root.join("home"),
        claude_dir: root.join("home/.claude"),
        claude_managed: root.join("managed/managed-settings.json"),
        codex_home: root.join("home/.codex"),
        codex_system: root.join("etc/codex"),
        codex_requirements: vec![root.join("etc/codex/requirements.toml")],
        agent_dir: Some(root.join("agent")),
    };
    Fx {
        agent: root.join("agent"),
        cwd,
        paths,
        _dir: dir,
    }
}

pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub fn claude_settings(path: &Path, permissions: Value) {
    write(path, &json!({ "permissions": permissions }).to_string());
}

/// Answers dialogs from queues and sends `keys` to each panel that opens.
#[derive(Default)]
pub struct Ui {
    pub selects: VecDeque<Value>,
    pub inputs: VecDeque<Value>,
    pub keys: VecDeque<Vec<&'static str>>,
    pub seen: Vec<Value>,
}

impl Ui {
    pub fn methods(&self, method: &str) -> Vec<&Value> {
        self.seen
            .iter()
            .filter(|m| m["method"] == method)
            .map(|m| &m["params"])
            .collect()
    }
}

pub async fn start(fx: &Fx) -> TestHost {
    let mut host = TestHost::start(extension(fx.paths.clone()))
        .cwd(fx.cwd.display().to_string())
        .agent_dir(fx.agent.display().to_string());
    host.initialize().await;
    host
}

pub fn ctx(fx: &Fx, ui: bool, trusted: bool) -> Value {
    json!({ "cwd": fx.cwd, "hasUI": ui, "flags": {}, "isProjectTrusted": trusted })
}

pub async fn drive(
    host: &mut TestHost,
    id: u64,
    method: &str,
    params: Value,
    ui: &mut Ui,
) -> Value {
    host.request(id, method, params).await;
    let mut panel = None;
    loop {
        let message = host.recv().await;
        let Some(name) = message["method"].as_str().map(str::to_owned) else {
            assert_eq!(message["id"], id, "{message}");
            return message;
        };
        ui.seen.push(message.clone());
        let request = message.get("id").cloned();
        match name.as_str() {
            "ui.custom" => {
                panel = request;
                let surface = message["params"]["surface"].clone();
                for key in ui.keys.pop_front().unwrap_or_else(|| vec!["\u{1b}"]) {
                    host.notify("ui.key", json!({ "surface": surface, "data": key }))
                        .await;
                }
            }
            "ui.close" => {
                if let Some(panel) = panel.take() {
                    host.answer(&panel, Value::Null).await;
                }
            }
            "ui.select" => {
                host.answer(
                    &request.unwrap(),
                    ui.selects.pop_front().unwrap_or(Value::Null),
                )
                .await
            }
            "ui.input" => {
                host.answer(
                    &request.unwrap(),
                    ui.inputs.pop_front().unwrap_or(Value::Null),
                )
                .await
            }
            _ => {
                if let Some(request) = request {
                    host.answer(&request, Value::Null).await;
                }
            }
        }
    }
}

pub async fn bash(host: &mut TestHost, id: u64, command: &str, ctx: Value, ui: &mut Ui) -> Value {
    let event = json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "bash", "input": { "command": command } });
    let response = drive(
        host,
        id,
        "event",
        json!({ "name": "tool_call", "event": event, "ctx": ctx }),
        ui,
    )
    .await;
    response["result"]["result"].clone()
}

pub fn blocked(result: &Value) -> Option<String> {
    (result["block"] == true).then(|| result["reason"].as_str().unwrap_or_default().to_owned())
}
