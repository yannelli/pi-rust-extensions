#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use pi_ext_sdk::test_util::TestHost;
use serde_json::{Value, json};

pub fn ctx(cwd: &str) -> Value {
    json!({ "cwd": cwd, "hasUI": true, "flags": {}, "model": { "provider": "p", "id": "m", "contextWindow": 200000 } })
}

pub fn tool(name: &str, path: &str) -> Value {
    json!({ "name": name, "description": format!("{name} tool"), "exposure": "direct",
            "parameters": { "type": "object" }, "sourceInfo": { "path": path, "source": "builtin" } })
}

/// A fake pi: `tools.*` over a shared loadout, `settings.get` from `setting`, and a session branch.
pub struct Fake {
    pub active: Arc<Mutex<Vec<String>>>,
    pub setting: Value,
    pub branch: Value,
    pub calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Fake {
    pub fn new(setting: Value, active: &[&str]) -> Self {
        let active = active.iter().map(|s| (*s).to_owned()).collect();
        Self {
            active: Arc::new(Mutex::new(active)),
            setting,
            branch: json!([]),
            calls: Arc::default(),
        }
    }

    pub fn answer(&self, method: &str, params: &Value) -> Value {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        match method {
            "settings.get" => self.setting.clone(),
            "session.getBranch" => self.branch.clone(),
            "tools.getAll" => json!([
                tool("read", "builtin:read"),
                tool("bash", "builtin:bash"),
                tool("mcp__gh__issue", "builtin:mcp"),
                tool("mcp__gh__pr", "builtin:mcp"),
                tool("deferred_search", "/ext/deferred"),
            ]),
            "tools.getActive" => json!(*self.active.lock().unwrap()),
            "tools.setActive" => {
                *self.active.lock().unwrap() =
                    serde_json::from_value(params["names"].clone()).unwrap();
                Value::Null
            }
            "pi.call" => json!(false),
            _ => Value::Null,
        }
    }

    pub fn active(&self) -> Vec<String> {
        self.active.lock().unwrap().clone()
    }

    pub fn calls_of(&self, method: &str) -> Vec<Value> {
        let calls = self.calls.lock().unwrap();
        calls
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

pub async fn event(
    host: &mut TestHost,
    fake: &Fake,
    id: u64,
    name: &str,
    event: Value,
    cwd: &str,
) -> Value {
    let params = json!({ "name": name, "event": event, "ctx": ctx(cwd) });
    host.drive(id, "event", params, |m, p| fake.answer(m, p))
        .await["result"]
        .clone()
}

pub async fn search(host: &mut TestHost, fake: &Fake, id: u64, args: Value, cwd: &str) -> Value {
    let params = json!({ "toolCallId": format!("c{id}"), "name": "deferred_search", "params": args, "ctx": ctx(cwd) });
    host.drive(id, "tool.execute", params, |m, p| fake.answer(m, p))
        .await["result"]
        .clone()
}

pub fn skill(dir: &str, name: &str) -> Value {
    let path = format!("{dir}/{name}/SKILL.md");
    json!({ "name": name, "description": format!("{name} skill"), "filePath": path, "baseDir": format!("{dir}/{name}"),
            "sourceInfo": { "path": path, "source": "local", "scope": "user", "origin": "top-level" } })
}

pub fn before(dir: &str) -> Value {
    json!({ "type": "before_agent_start", "prompt": "hi", "systemPromptOptions": {
        "skills": [skill(dir, "pdf"), skill(dir, "docx")],
        "contextFiles": [{ "path": format!("{dir}/AGENTS.md"), "content": "# House rules\nTabs." }],
        "sections": { "mcp_servers": "gh: GitHub" }
    } })
}
