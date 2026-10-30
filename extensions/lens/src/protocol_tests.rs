use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use std::collections::VecDeque;
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

use super::Project;

struct Host {
    writer: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    task: tokio::task::JoinHandle<pi_ext_sdk::Result<()>>,
    calls: Vec<Value>,
    answers: VecDeque<Value>,
}

impl Host {
    fn new(extension: Extension) -> Self {
        let (host, extension_io) = tokio::io::duplex(65536);
        let (read, write) = tokio::io::split(extension_io);
        let task = tokio::spawn(extension.serve(read, write));
        let (read, writer) = tokio::io::split(host);
        Self {
            writer,
            lines: BufReader::new(read).lines(),
            task,
            calls: vec![],
            answers: VecDeque::new(),
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let request = json!({"jsonrpc":"2.0","id":"test","method":method,"params":params});
        self.writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        loop {
            let line =
                tokio::time::timeout(std::time::Duration::from_secs(5), self.lines.next_line())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == "test" {
                assert!(value.get("error").is_none(), "{value}");
                return value["result"].clone();
            }
            let result = match value["method"].as_str() {
                Some("session.getBranch") => json!([]),
                Some("pi.call") => json!(true),
                Some("ui.select" | "ui.editor" | "ui.confirm") => {
                    self.answers.pop_front().unwrap_or_default()
                }
                _ => Value::Null,
            };
            self.calls.push(value.clone());
            if value.get("id").is_some() {
                let response = json!({"jsonrpc":"2.0","id":value["id"],"result":result});
                self.writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        }
    }

    async fn event(&mut self, name: &str, event: Value, project: &Project) -> Value {
        self.request("event", json!({"name":name,"event":event,"ctx":{"cwd":project.0,"hasUI":false,"isProjectTrusted":true}})).await
    }

    async fn finish(mut self) {
        self.writer.shutdown().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn lens_menu_saves_language_tool_command_and_lsp_settings() {
    let project = Project::new();
    let mut host = Host::new(crate::extension());
    host.request("initialize", json!({"protocolVersion":1,"cwd":project.0,"agentDir":project.0.join("agent"),"hasUI":true})).await;
    host.answers = [
        json!("Languages"),
        json!("Project"),
        json!("python"),
        json!("Commands"),
        json!("Project"),
        json!("Add"),
        json!(
            r#"{"name":"fake","language":"python","stage":"lint","command":"ruff","enabled":true}"#
        ),
        json!("Commands"),
        json!("Project"),
        json!("1: fake [on]"),
        json!("Enable or disable"),
        json!("LSP servers"),
        json!("Global"),
        json!("Add"),
        json!(r#"{"command":"rust-analyzer","languages":["rust"]}"#),
        json!("Tools"),
        json!("Global"),
        json!("ruff"),
        Value::Null,
    ]
    .into();
    host.request("command.run", json!({"name":"lens","args":"","ctx":{"cwd":project.0,"hasUI":true,"isProjectTrusted":true}})).await;
    assert!(host.answers.is_empty());
    let project_config: Value =
        serde_json::from_slice(&std::fs::read(project.0.join(".pi/ext/lens.json")).unwrap())
            .unwrap();
    assert_eq!(project_config["languages"]["python"]["enabled"], false);
    assert_eq!(project_config["commands"][0]["enabled"], false);
    let global_config: Value =
        serde_json::from_slice(&std::fs::read(project.0.join("agent/ext/lens.json")).unwrap())
            .unwrap();
    assert_eq!(
        global_config["lsp"]["servers"][0]["command"],
        "rust-analyzer"
    );
    assert_eq!(global_config["tools"]["ruff"]["enabled"], false);
    host.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn guard_pipeline_context_and_reset_use_the_wire_protocol() {
    let project = Project::new();
    project.write("agent/ext/lens.json", r#"{"autoDetect":false,"commands":[{"name":"fake","language":"python","stage":"lint","command":"/bin/sh","args":["-c","printf 'a.py:2:3: error: known diagnostic\\n'"]}]}"#);
    project.write("a.py", "first\nsecond\n");
    let mut host = Host::new(crate::extension());
    let registrations = host.request("initialize", json!({"protocolVersion":1,"cwd":project.0,"agentDir":project.0.join("agent"),"hasUI":false})).await;
    assert!(
        registrations["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "lens_diagnostics")
    );
    host.event("session_start", json!({}), &project).await;
    let edit = json!({"toolName":"edit","input":{"path":"a.py","oldText":"second"}});
    assert_eq!(
        host.event("tool_call", edit.clone(), &project).await["result"]["block"],
        true
    );
    host.event("tool_result", json!({"toolName":"read","input":{"path":"a.py"},"content":[{"type":"text","text":"first\nsecond"}],"isError":false}), &project).await;
    assert!(
        host.event("tool_call", edit.clone(), &project)
            .await
            .get("result")
            .is_none()
    );
    host.event(
        "tool_result",
        json!({"toolName":"edit","input":{"path":"a.py"},"isError":false}),
        &project,
    )
    .await;
    let context = host
        .event(
            "context",
            json!({"messages":[{"role":"user","content":"hello"}]}),
            &project,
        )
        .await;
    assert_eq!(context["result"]["messages"].as_array().unwrap().len(), 2);
    assert!(context.to_string().contains("known diagnostic"));
    let tool = host.request("tool.execute", json!({"name":"lens_diagnostics","params":{"severity":"error"},"toolCallId":"diag","ctx":{"cwd":project.0}})).await;
    assert_eq!(tool["details"]["findings"][0]["column"], 3);
    assert_eq!(
        host.event("tool_call", edit, &project).await["result"]["block"],
        true
    );
    host.event("session_tree", json!({}), &project).await;
    let context = host
        .event("context", json!({"messages":[]}), &project)
        .await;
    assert!(context.get("result").is_none());
    assert!(
        host.calls
            .iter()
            .any(|call| call["method"] == "session.appendEntry")
    );
    host.finish().await;
}
