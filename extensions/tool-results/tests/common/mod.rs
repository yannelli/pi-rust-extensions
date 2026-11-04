#![allow(dead_code)]

use pi_ext_sdk::test_util::TestHost;
use pi_ext_tool_results::extension;
use serde_json::{Value, json};

/// A scripted pi for one session: `session` is the session id, `entries` what
/// `session.getEntries` answers, and `sent` every request the extension made.
pub struct Run {
    pub host: TestHost,
    pub root: String,
    pub session: String,
    pub next: u64,
    pub entries: Value,
    pub notes: Vec<String>,
    pub sent: Vec<(String, Value)>,
}

impl Run {
    /// `features`: the `features` of `initialize`, such as `toolResultExpander`.
    pub async fn start(dir: &std::path::Path, session: &str, features: &[&str]) -> Self {
        let root = dir.display().to_string();
        let mut host = TestHost::start(extension());
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.13.0", "cwd": root, "agentDir": root,
            "extensionPath": "native:test/test", "platform": "linux", "mode": "tui",
            "hasUI": false, "flags": {}, "features": features
        });
        host.request(0, "initialize", params).await;
        let init = host.expect_response(0).await["result"].clone();
        host.notify("initialized", json!({})).await;
        assert_eq!(
            init["events"],
            json!(["session_start", "session_shutdown", "tool_result"])
        );
        assert_eq!(init["tools"][0]["name"], "tool_result_get");
        assert_eq!(
            init["toolResultExpander"],
            json!({ "detailsKey": "toolResults", "maxHeight": 30 })
        );
        let mut run = Self {
            host,
            root,
            session: session.to_owned(),
            next: 1,
            entries: json!([]),
            notes: Vec::new(),
            sent: Vec::new(),
        };
        let start = json!({ "type": "session_start", "reason": "startup" });
        assert_eq!(run.event("session_start", start).await, json!({}));
        run
    }

    pub fn ctx(&self) -> Value {
        json!({ "cwd": self.root, "hasUI": false, "flags": {}, "sessionId": self.session,
                "sessionFile": format!("{}/{}.jsonl", self.root, self.session), "isProjectTrusted": false })
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let (entries, mut sent) = (self.entries.clone(), Vec::new());
        let reply = self
            .host
            .drive(self.next, method, params, |method, params| {
                sent.push((method.to_owned(), params.clone()));
                match method {
                    "pi.call" => json!(false),
                    "session.getEntries" => entries.clone(),
                    _ => Value::Null,
                }
            })
            .await;
        for (method, params) in &sent {
            if method == "ui.notify" {
                self.notes
                    .push(params["message"].as_str().unwrap_or_default().to_owned());
            }
        }
        self.sent.extend(sent);
        reply["result"].clone()
    }

    pub async fn event(&mut self, name: &str, event: Value) -> Value {
        let params = json!({ "name": name, "event": event, "ctx": self.ctx() });
        self.call("event", params).await
    }

    pub async fn result(&mut self, tool: &str, id: &str, content: Value, is_error: bool) -> Value {
        let event = json!({ "type": "tool_result", "toolName": tool, "toolCallId": id,
                            "input": {}, "content": content, "details": { "exitCode": 0 }, "isError": is_error });
        self.event("tool_result", event).await
    }

    pub async fn get(&mut self, args: Value) -> Value {
        let params = json!({ "toolCallId": "g", "name": "tool_result_get", "params": args, "ctx": self.ctx() });
        self.call("tool.execute", params).await
    }

    pub async fn command(&mut self, args: &str) -> String {
        let params = json!({ "name": "tool-results", "args": args, "ctx": self.ctx() });
        self.call("command.run", params).await;
        self.notes.last().cloned().unwrap_or_default()
    }
}

pub fn text(reply: &Value) -> &str {
    reply["patch"]["content"][0]["text"].as_str().unwrap()
}

/// The result id in a truncation notice.
pub fn notice_id(shown: &str) -> String {
    shown
        .split("result_id \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
