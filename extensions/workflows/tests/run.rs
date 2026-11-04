//! A workflow run end to end: the `pi-rust-ext workflows` process over the protocol, with
//! `PI_BIN` pointing at a fake pi that records its arguments and answers with scripted reports.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use pi_ext_e2e::{MockProvider, TempDir, Turn, Value, cargo_bin, json, pi_bin_or_skip};

const FAKE_PI: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
n=$(ls "$dir" | grep -c '^call-')
printf '%s\0' "$@" > "$dir/call-$n"
for a in "$@"; do last="$a"; done
status=done
case "$last" in
  *FAIL*) echo boom >&2; exit 1 ;;
  *"Watch CI"*) if [ -f "$dir/ci-seen" ]; then status=done; else touch "$dir/ci-seen"; status=continue; fi ;;
esac
echo '{"type":"turn_start"}'
echo '{"type":"tool_execution_start","toolName":"bash","args":{"command":"make"}}'
echo '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"x"}],"provider":"m","model":"one","usage":{"cost":{"total":0.5}}}}'
printf '{"type":"output","value":{"status":"%s","summary":"did call %s"}}\n' "$status" "$n"
"#;

struct Ext {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    next_id: u64,
    ctx: Value,
    /// `session.sendMessage` params the extension sent.
    messages: Vec<Value>,
}

impl Ext {
    fn start(pi: &Path, agent_dir: &Path, cwd: &Path, env: &[(&str, &Path)]) -> Self {
        let mut child = Command::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
            .arg("workflows")
            .env("PI_BIN", pi)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = reader(child.stdout.take().unwrap());
        let ctx = json!({ "cwd": cwd, "hasUI": false, "flags": {}, "model": { "provider": "p", "id": "parent" },
            "thinkingLevel": "high" });
        let mut ext = Self {
            child,
            stdin,
            lines,
            next_id: 1,
            ctx,
            messages: Vec::new(),
        };
        let init = json!({ "protocolVersion": 1, "piVersion": "0.12.9", "cwd": cwd, "agentDir": agent_dir,
            "extensionPath": "native:test/workflows", "platform": "linux", "mode": "print", "hasUI": false, "flags": {} });
        let result = ext.call("initialize", init);
        assert!(
            result["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "workflow"),
            "{result}"
        );
        ext.send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
        let event = json!({ "name": "session_start", "event": { "type": "session_start", "reason": "startup" }, "ctx": ext.ctx });
        ext.call("event", event);
        ext
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
    }

    /// Sends a request and answers the extension's requests until the response arrives.
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self.recv();
            if message.get("method").is_none() && message["id"] == id {
                assert!(message.get("error").is_none(), "{message}");
                return message["result"].clone();
            }
            self.answer(&message);
        }
    }

    fn recv(&mut self) -> Value {
        self.lines
            .recv_timeout(Duration::from_secs(20))
            .expect("the extension stopped answering")
    }

    fn answer(&mut self, message: &Value) {
        let Some(id) = message.get("id").cloned() else {
            return;
        };
        let result = match message["method"].as_str().unwrap_or("") {
            "commands.getAll" => {
                json!([{ "name": "skill:review", "source": "skill", "sourceInfo": { "path": "/skills/review/SKILL.md" } }])
            }
            "tools.getAll" => json!([{ "name": "read" }, { "name": "mcp__github__issues" }]),
            "session.sendMessage" => {
                self.messages.push(message["params"].clone());
                Value::Null
            }
            _ => Value::Null,
        };
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn tool(&mut self, params: Value) -> String {
        let call = json!({ "toolCallId": format!("t{}", self.next_id), "name": "workflow", "params": params, "ctx": self.ctx });
        let result = self.call("tool.execute", call);
        assert_ne!(result["isError"], true, "{result}");
        result["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }

    /// Answers requests until the run reports its end.
    fn wait_for_report(&mut self) -> Value {
        while self.messages.is_empty() {
            let message = self.recv();
            self.answer(&message);
        }
        self.messages.remove(0)
    }
}

impl Drop for Ext {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reader(stdout: ChildStdout) -> Receiver<Value> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str(&line)
                && tx.send(value).is_err()
            {
                break;
            }
        }
    });
    rx
}

/// The arguments of each fake pi call, in call order.
fn calls(dir: &Path) -> Vec<Vec<String>> {
    let mut files: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let n = e
                .file_name()
                .to_str()?
                .strip_prefix("call-")?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|(_, path)| {
            let text = std::fs::read_to_string(path).unwrap();
            text.trim_end_matches('\0')
                .split('\0')
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .map(|i| args[i + 1].as_str())
}

#[test]
fn runs_steps_with_scopes_goals_handoff_and_optional_failures() {
    let root = TempDir::new().unwrap();
    let fake_dir = root.path().join("fake");
    let (agent, work) = (root.path().join("agent"), root.path().join("work"));
    for dir in [&fake_dir, &agent, &work] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let fake_pi = fake_dir.join("pi");
    std::fs::write(&fake_pi, FAKE_PI).unwrap();
    std::fs::set_permissions(&fake_pi, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut ext = Ext::start(&fake_pi, &agent, &work, &[]);

    let workflow = json!({
        "name": "flow",
        "description": "Build and ship.",
        "steps": [
            { "name": "build", "agent": { "model": "m/one", "tools": ["read"], "mcp": ["github"] },
              "tasks": [{ "prompt": "Build {{thing}}" }] },
            { "name": "ci", "tasks": [
                { "prompt": "Watch CI", "until": "green", "every": "0s", "repeat": 3 },
                { "prompt": "FAIL please", "optional": true },
                { "prompt": "Review", "skill": "review" }
            ] }
        ]
    });
    let saved = ext.tool(json!({ "action": "save", "workflow": workflow }));
    assert!(saved.contains("Inputs: thing"), "{saved}");
    let started =
        ext.tool(json!({ "action": "run", "name": "flow", "inputs": { "thing": "the app" } }));
    let id = started
        .split_whitespace()
        .nth(2)
        .unwrap()
        .trim_end_matches('.')
        .to_owned();

    let report = ext.wait_for_report();
    let text = report["content"].as_str().unwrap();
    assert!(text.starts_with("Workflow **flow** finished"), "{text}");
    assert!(text.contains("✓ ci: did call 4"), "{text}");
    assert_eq!(
        report["triggerTurn"], true,
        "agent-started runs wake the agent: {report}"
    );

    let calls = calls(&fake_dir);
    assert_eq!(calls.len(), 5, "build, CI twice, the failing task, review");
    let build = &calls[0];
    assert_eq!(after(build, "--model"), Some("m/one"));
    assert_eq!(
        after(build, "--thinking"),
        None,
        "a step model does not take the parent's thinking"
    );
    assert_eq!(after(build, "--tools"), Some("read,mcp__github__*"));
    assert!(
        after(build, "--output-schema")
            .unwrap()
            .ends_with("report.schema.json")
    );
    let prompt = build.last().unwrap();
    assert!(
        prompt.starts_with("# Workflow flow: step 1 of 2, \"build\"\nBuild and ship."),
        "{prompt}"
    );
    assert!(prompt.ends_with("## Your task\nBuild the app"), "{prompt}");

    let ci = &calls[1];
    assert_eq!(after(ci, "--model"), Some("p/parent"));
    assert_eq!(after(ci, "--thinking"), Some("high"));
    let ci_prompt = ci.last().unwrap();
    assert!(
        ci_prompt.contains("### 1. build (done)\ndid call 0"),
        "{ci_prompt}"
    );
    assert!(ci_prompt.contains("Goal: green"), "{ci_prompt}");
    assert!(
        calls[2]
            .last()
            .unwrap()
            .starts_with("Run 2 of 3. Check again."),
        "{:?}",
        calls[2].last()
    );
    let session = after(ci, "--session").unwrap();
    assert!(session.ends_with("02-ci.jsonl"), "{session}");
    assert!(
        calls[2..]
            .iter()
            .all(|c| after(c, "--session") == Some(session)),
        "one session per step"
    );
    assert_eq!(
        calls[4].last().map(String::as_str),
        Some("/skill:review Review")
    );

    let status = ext.tool(json!({ "action": "status", "id": id }));
    assert!(status.contains("2. ci · done"), "{status}");
    assert!(status.contains("4 agent runs"), "{status}");
    assert!(
        status.contains("ci 2/3: failed: pi exited with code 1: boom"),
        "{status}"
    );
    assert!(
        status.contains("The task is optional, so the workflow continues."),
        "{status}"
    );
    let run_file = agent.join("ext/workflows/runs").join(&id).join("run.json");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(run_file).unwrap()).unwrap();
    assert_eq!(saved["status"], "done");
    assert_eq!(saved["steps"][1]["tasksDone"], 3);
}

#[test]
fn blocked_report_fails_the_run_and_resume_continues_it() {
    let root = TempDir::new().unwrap();
    let fake_dir = root.path().join("fake");
    let (agent, work) = (root.path().join("agent"), root.path().join("work"));
    for dir in [&fake_dir, &agent, &work] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let fake_pi = fake_dir.join("pi");
    let blocking = FAKE_PI.replace(
        "status=done\ncase",
        "status=done\n[ -f \"$dir/unblock\" ] || status=blocked\ncase",
    );
    std::fs::write(&fake_pi, blocking).unwrap();
    std::fs::set_permissions(&fake_pi, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut ext = Ext::start(&fake_pi, &agent, &work, &[]);

    let workflow = json!({ "name": "deploy", "steps": [
        { "name": "migrate", "tasks": [{ "prompt": "Run migrations" }] },
        { "name": "smoke", "tasks": [{ "prompt": "Smoke test" }] }
    ] });
    let started = ext.tool(json!({ "action": "run", "workflow": workflow }));
    let id = started
        .split_whitespace()
        .nth(2)
        .unwrap()
        .trim_end_matches('.')
        .to_owned();
    let report = ext.wait_for_report();
    let text = report["content"].as_str().unwrap();
    assert!(text.contains("Workflow **deploy** failed"), "{text}");
    assert!(
        text.contains("Error: migrate is blocked: did call 0"),
        "{text}"
    );

    std::fs::write(fake_dir.join("unblock"), "").unwrap();
    let resumed = ext.tool(json!({ "action": "resume", "id": id }));
    assert_eq!(resumed, format!("Resumed run {id}."));
    let report = ext.wait_for_report();
    let text = report["content"].as_str().unwrap();
    assert!(text.contains("Workflow **deploy** finished"), "{text}");
    let calls = calls(&fake_dir);
    assert_eq!(calls.len(), 3, "migrate twice, then smoke");
    assert_eq!(
        after(&calls[0], "--session"),
        after(&calls[1], "--session"),
        "resume continues the session"
    );
    assert!(
        calls[1].last().unwrap().starts_with("Run migrations"),
        "no second header: {:?}",
        calls[1].last()
    );
}

#[test]
fn real_pi_runs_a_step_and_returns_its_report() {
    let Some(pi) = pi_bin_or_skip() else { return };
    let root = TempDir::new().unwrap();
    let (agent, work, child_agent) = (
        root.path().join("agent"),
        root.path().join("work"),
        root.path().join("child"),
    );
    for dir in [&agent, &work, &child_agent] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let provider = MockProvider::start(vec![Turn::text(
        r#"{"status": "done", "summary": "real pi ok"}"#,
    )])
    .unwrap();
    let models = json!({ "providers": { "mock": { "baseUrl": provider.url(), "api": "openai-completions", "apiKey": "k",
        "models": [{ "id": "mock-model", "contextWindow": 100000, "maxTokens": 4000 }] } } });
    std::fs::write(child_agent.join("models.json"), models.to_string()).unwrap();
    let mut ext = Ext::start(&pi, &agent, &work, &[("PI_CODING_AGENT_DIR", &child_agent)]);
    ext.ctx["model"] = json!({ "provider": "mock", "id": "mock-model" });

    let workflow = json!({ "name": "check", "steps": [
        { "name": "check", "agent": { "tools": ["read"], "maxTurns": 3 }, "tasks": [{ "prompt": "Say ok" }] }
    ] });
    ext.tool(json!({ "action": "run", "workflow": workflow }));
    let report = ext.wait_for_report();
    let text = report["content"].as_str().unwrap();
    assert!(text.contains("✓ check: real pi ok"), "{text}");
    let requests = provider.requests();
    let request = requests[0].to_string();
    assert!(
        request.contains("You run one step of an automated workflow"),
        "{request}"
    );
    assert!(request.contains("Say ok"), "{request}");
}

#[test]
fn pi_loads_the_tool_and_skills_and_saves_through_the_tool() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let workflow = json!({ "name": "Ship It", "steps": [{ "name": "build", "tasks": [{ "prompt": "Build {{thing}}" }] }] });
    let output = pi_ext_e2e::Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["workflows"])
        .turns([
            Turn::tool_call("workflow", json!({ "action": "save", "workflow": workflow })),
            Turn::tool_call("workflow", json!({ "action": "save", "workflow": { "name": "bad", "steps": [{ "name": "x", "tasks": [], "modle": "a/b" }] } })),
            Turn::text("Saved."),
        ])
        .prompt("Make a workflow")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let results = output.tool_results("workflow");
    let text = |i: usize| {
        results[i]["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    };
    assert!(text(0).starts_with("Saved ship-it to "), "{}", text(0));
    assert!(text(0).ends_with("Inputs: thing."), "{}", text(0));
    assert!(text(1).contains("unknown field `modle`"), "{}", text(1));
    let prompt = output.system_prompt(0);
    assert!(
        prompt.contains("workflow-create") && prompt.contains("workflow-optimize"),
        "{prompt}"
    );
    assert!(output.tool_names(0).contains(&"workflow".to_owned()));
}
