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
sleep "${FAKE_PI_SLEEP:-0}"
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
    /// The answer to `ui.select` dialogs; `None` dismisses them.
    select: Option<String>,
}

impl Ext {
    /// The extension in a pi session; `ui` is `hasUI` of the session.
    fn start(pi: &Path, agent_dir: &Path, cwd: &Path, env: &[(&str, &str)], ui: bool) -> Self {
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
        let ctx = json!({ "cwd": cwd, "hasUI": ui, "flags": {}, "model": { "provider": "p", "id": "parent" },
            "thinkingLevel": "high" });
        let mut ext = Self {
            child,
            stdin,
            lines,
            next_id: 1,
            ctx,
            messages: Vec::new(),
            select: None,
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
            "ui.select" => json!(self.select),
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
    let mut ext = Ext::start(&fake_pi, &agent, &work, &[], false);

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
    let mut ext = Ext::start(&fake_pi, &agent, &work, &[], false);

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
    let child_dir = child_agent.display().to_string();
    let mut ext = Ext::start(
        &pi,
        &agent,
        &work,
        &[("PI_CODING_AGENT_DIR", &child_dir)],
        false,
    );
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

#[test]
fn a_message_in_the_inbox_steers_the_step_agent() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let root = TempDir::new().unwrap();
    let inbox = root.path().join("inbox");
    let writer = {
        let inbox = inbox.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let message = serde_json::to_string("Use the staging branch").unwrap();
            std::fs::create_dir_all(&inbox).unwrap();
            std::fs::write(inbox.join("0000000000001-1-0.json"), message).unwrap();
        })
    };
    let output = pi_ext_e2e::Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["workflows"])
        .env("PI_WORKFLOW_RUN", "r1")
        .env("PI_WORKFLOW_INBOX", inbox.display().to_string())
        .turns([
            Turn::tool_call("bash", json!({ "command": "sleep 4" })),
            Turn::text("Done on staging."),
        ])
        .prompt("Deploy")
        .run()
        .unwrap();
    writer.join().unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let second = output.requests[1].to_string();
    assert!(second.contains("Use the staging branch"), "{second}");
    assert!(std::fs::read_dir(&inbox).unwrap().next().is_none());
}

struct Setup {
    root: TempDir,
    fake_dir: PathBuf,
    fake_pi: PathBuf,
    agent: PathBuf,
    work: PathBuf,
}

fn setup() -> Setup {
    let root = TempDir::new().unwrap();
    let fake_dir = root.path().join("fake");
    let (agent, work) = (root.path().join("agent"), root.path().join("work"));
    for dir in [&fake_dir, &agent, &work] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let fake_pi = fake_dir.join("pi");
    std::fs::write(&fake_pi, FAKE_PI).unwrap();
    std::fs::set_permissions(&fake_pi, std::fs::Permissions::from_mode(0o755)).unwrap();
    Setup {
        root,
        fake_dir,
        fake_pi,
        agent,
        work,
    }
}

impl Setup {
    fn ext(&self, env: &[(&str, &str)], ui: bool) -> Ext {
        Ext::start(&self.fake_pi, &self.agent, &self.work, env, ui)
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        self.agent.join("ext/workflows/runs").join(id)
    }

    /// Reads `run.json` until `done` holds for it.
    fn wait_run(&self, id: &str, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let text =
                std::fs::read_to_string(self.run_dir(id).join("run.json")).unwrap_or_default();
            let run: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if done(&run) {
                return run;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run is not {what}: {run}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The run id in the text of the `run` action.
fn run_id(started: &str) -> String {
    let id = started.split_whitespace().nth(2).unwrap();
    id.trim_end_matches('.').to_owned()
}

#[test]
fn background_run_outlives_its_session_and_a_later_session_reports_it() {
    let s = setup();
    let mut first = s.ext(&[("FAKE_PI_SLEEP", "2")], false);
    let workflow = json!({ "name": "nightly", "steps": [{ "name": "check", "tasks": [{ "prompt": "Check" }] }] });
    let started = first.tool(json!({ "action": "run", "workflow": workflow, "background": true }));
    assert!(started.contains("in the background"), "{started}");
    let id = run_id(&started);
    drop(first);
    let run = s.wait_run(&id, "done", |r| r["status"] == "done");
    assert_eq!(run["background"], true);
    assert!(run["heartbeatMs"].as_u64().unwrap() > 0);

    let mut later = s.ext(&[], true);
    let report = later.wait_for_report();
    let text = report["content"].as_str().unwrap();
    assert!(text.starts_with("Workflow **nightly** finished"), "{text}");
    let claims: Vec<_> = std::fs::read_dir(s.run_dir(&id))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("reported-"))
        .collect();
    assert_eq!(claims.len(), 1, "one session claims the report");
}

#[test]
fn foreground_run_stops_when_its_pi_quits_and_switches_to_background() {
    let s = setup();
    let mut ext = s.ext(&[("FAKE_PI_SLEEP", "30")], true);
    let workflow =
        json!({ "name": "long", "steps": [{ "name": "wait", "tasks": [{ "prompt": "Wait" }] }] });
    let started = ext.tool(json!({ "action": "run", "workflow": workflow }));
    assert!(started.contains("in the foreground"), "{started}");
    let id = run_id(&started);
    s.wait_run(&id, "running its agent", |r| r["steps"][0]["runs"] == 1);
    let switched = ext.tool(json!({ "action": "background", "id": id }));
    assert!(switched.contains("background now"), "{switched}");
    s.wait_run(&id, "in the background", |r| r["background"] == true);
    ext.tool(json!({ "action": "foreground", "id": id }));
    s.wait_run(&id, "in the foreground", |r| r["background"] == false);

    let event = json!({ "type": "session_shutdown", "reason": "quit" });
    ext.call(
        "event",
        json!({ "name": "session_shutdown", "event": event, "ctx": ext.ctx }),
    );
    ext.call("shutdown", json!({}));
    let run = s.wait_run(&id, "interrupted", |r| r["status"] == "interrupted");
    assert_eq!(
        run["steps"][0]["status"], "pending",
        "resume starts the task again"
    );
}

#[test]
fn approvals_wait_without_a_ui_and_any_session_can_answer() {
    let s = setup();
    let mut headless = s.ext(&[], false);
    let workflow = json!({ "name": "deploy", "steps": [{ "name": "ship", "tasks": [
        { "prompt": "Merge", "confirm": true },
        { "prompt": "Deploy", "confirm": true }
    ] }] });
    let id = run_id(&headless.tool(json!({ "action": "run", "workflow": workflow })));
    let run = s.wait_run(&id, "waiting for approval", |r| {
        r["approval"]["task"] == "ship 1/2"
    });
    assert_eq!(run["status"], "waiting");
    let command = json!({ "name": "workflows", "args": "approve", "ctx": headless.ctx });
    headless.call("command.run", command);
    s.wait_run(&id, "waiting for the second approval", |r| {
        r["approval"]["task"] == "ship 2/2"
    });
    drop(headless);

    let mut tui = s.ext(&[], true);
    tui.select = Some("Run it".into());
    let report = tui.wait_for_report();
    let text = report["content"].as_str().unwrap();
    assert!(text.contains("Workflow **deploy** finished"), "{text}");
    let run = s.wait_run(&id, "done", |r| r["status"] == "done");
    let log = run["steps"][0]["log"].to_string();
    assert!(log.contains("ship 1/2: approved by the user"), "{log}");
    assert!(log.contains("ship 2/2: approved by the user"), "{log}");
    assert_eq!(calls(&s.fake_dir).len(), 2);
    drop(s.root);
}
