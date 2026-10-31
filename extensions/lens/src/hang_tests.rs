#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::Project;
use super::protocol::Host;
use crate::config::{Config, LspServer, Stage, ToolCommand};
use crate::group::{Group, Live};
use crate::pipeline::{self, Runner};
use crate::state::{Guard, MAX_GUARD_BYTES, small_text};

fn shell(name: &str, script: &str) -> ToolCommand {
    ToolCommand {
        name: name.into(),
        language: "python".into(),
        stage: Stage::Lint,
        command: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        enabled: true,
    }
}

fn config(commands: Vec<ToolCommand>, servers: Vec<LspServer>) -> Config {
    let mut config = Config {
        auto_detect: false,
        commands,
        ..Default::default()
    };
    config.lsp.servers = servers;
    config
}

fn stalled_server() -> LspServer {
    LspServer {
        command: "/bin/sh".into(),
        args: vec!["-c".into(), "exec sleep 30".into()],
        languages: vec!["python".into()],
        enabled: true,
    }
}

async fn scan(config: &Config, project: &Project, runner: &Runner) -> (Vec<Value>, Duration) {
    let file = project.0.join("a.py");
    if !file.exists() {
        project.write("a.py", "x = 1\n");
    }
    let started = Instant::now();
    let findings = pipeline::run(config, &project.0, &file, runner).await;
    (findings, started.elapsed())
}

fn messages(findings: &[Value]) -> Vec<&str> {
    findings
        .iter()
        .filter_map(|item| item["message"].as_str())
        .collect()
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn assert_dead(pid: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive(pid), "{pid} is still alive");
}

#[tokio::test]
async fn tool_that_leaves_a_process_on_its_output_returns_and_stops_it() {
    let project = Project::new();
    let script = "echo 'a.py:1:1: error: daemon-finding'; sleep 30 & echo $! > pid";
    let config = config(vec![shell("daemon", script)], vec![]);
    let (findings, elapsed) = scan(&config, &project, &Runner::default()).await;
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert_eq!(messages(&findings), ["error: daemon-finding"]);
    assert_dead(&std::fs::read_to_string(project.0.join("pid")).unwrap()).await;
}

#[tokio::test]
async fn non_utf8_huge_and_stdin_reading_tools_do_not_hang_or_panic() {
    let project = Project::new();
    let commands = vec![
        shell("bytes", "printf '\\377\\376 bad\\n'; exit 1"),
        shell("line", "head -c 3000000 /dev/zero | tr '\\0' x; exit 1"),
        shell("spam", "yes 'a.py:1:1: error: spam' | head -c 5000000"),
        shell("stdin", "read line; echo \"got [$line]\"; exit 1"),
    ];
    let config = config(commands, vec![]);
    let (findings, elapsed) = scan(&config, &project, &Runner::default()).await;
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    let text = messages(&findings);
    assert!(text[0].contains('\u{FFFD}'), "{}", text[0]);
    assert!(text[1].len() <= crate::diagnostics::MAX_MESSAGE_BYTES);
    assert_eq!(
        findings
            .iter()
            .filter(|item| item["source"] == "spam")
            .count(),
        201
    );
    assert!(
        text.iter()
            .any(|item| item.contains("more findings not shown"))
    );
    assert!(text.contains(&"got []"), "{text:?}");
}

#[tokio::test]
async fn missing_binary_is_a_warning() {
    let project = Project::new();
    let mut tool = shell("missing", "");
    tool.command = "/nonexistent/lens-tool".into();
    let (findings, elapsed) = scan(&config(vec![tool], vec![]), &project, &Runner::default()).await;
    assert!(elapsed < Duration::from_secs(1));
    assert_eq!(findings[0]["severity"], "warning");
    assert!(messages(&findings)[0].contains("/nonexistent/lens-tool"));
}

#[tokio::test]
async fn stalled_server_is_skipped_after_one_timeout() {
    let project = Project::new();
    let mut config = config(vec![], vec![stalled_server()]);
    config.timeout_ms = 300;
    let runner = Runner::default();
    let (first, _) = scan(&config, &project, &runner).await;
    assert!(messages(&first)[0].contains("timed out"), "{first:?}");
    let (second, elapsed) = scan(&config, &project, &runner).await;
    assert!(messages(&second)[0].starts_with("Skipped: timed out earlier"));
    assert!(elapsed < Duration::from_millis(200), "{elapsed:?}");
    runner.reset();
    let (third, _) = scan(&config, &project, &runner).await;
    assert!(messages(&third)[0].contains("timed out"));
    config.pipeline_timeout_ms = 300;
    let runner = Runner::default();
    scan(&config, &project, &runner).await;
    let (equal, _) = scan(&config, &project, &runner).await;
    assert!(
        messages(&equal)[0].starts_with("Skipped: timed out earlier"),
        "{equal:?}"
    );
}

#[tokio::test]
async fn concurrent_scans_run_one_at_a_time() {
    let project = Project::new();
    let script = "echo start >> order; sleep 0.3; echo end >> order";
    let config = config(vec![shell("slow", script)], vec![]);
    let runner = Runner::default();
    let (a, b) = tokio::join!(
        scan(&config, &project, &runner),
        scan(&config, &project, &runner)
    );
    let order = std::fs::read_to_string(project.0.join("order")).unwrap();
    assert_eq!(order, "start\nend\nstart\nend\n", "{a:?} {b:?}");
}

#[test]
fn guard_passes_fifos_large_and_binary_files() {
    let project = Project::new();
    let fifo = project.0.join("pipe.py");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let large = project.write("large.py", &"x".repeat(MAX_GUARD_BYTES as usize + 1));
    let binary = project.0.join("binary.py");
    std::fs::write(&binary, b"\xff\xfe\x00").unwrap();
    let mut guard = Guard::default();
    for path in [&fifo, &large, &binary] {
        assert!(small_text(path).is_none());
        assert!(
            guard
                .block_reason(path, small_text(path), "write", &json!({}))
                .is_none()
        );
    }
}

#[tokio::test]
async fn kill_all_stops_groups_that_were_never_dropped() {
    let project = Project::new();
    let pid_file = project.0.join("pid");
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args(["-c", "sleep 30 & echo $! > \"$0\"; wait"])
        .arg(&pid_file)
        .process_group(0);
    let child = command.spawn().unwrap();
    let live = Live::default();
    std::mem::forget(Group::new(child.id(), &live));
    while !pid_file.exists() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(live.kill_all(), 1);
    assert_dead(&std::fs::read_to_string(&pid_file).unwrap()).await;
}

fn lens_config(project: &Project) {
    let config = json!({"autoDetect":false,"timeoutMs":400,"pipelineTimeoutMs":700,
        "lsp":{"servers":[{"command":"/bin/sh","args":["-c","exec sleep 30"],"languages":["python"]}]}});
    project.write("agent/ext/lens.json", &config.to_string());
}

async fn started_host(project: &Project) -> Host {
    let mut host = Host::new(crate::extension());
    host.request("initialize", json!({"protocolVersion":1,"cwd":project.0,"agentDir":project.0.join("agent"),"hasUI":false})).await;
    host.event("session_start", json!({}), project).await;
    host
}

#[tokio::test]
async fn hooks_with_a_stalled_server_answer_within_the_limit() {
    let project = Project::new();
    lens_config(&project);
    for name in ["a.py", "b.py", "c.py"] {
        project.write(name, "x = 1\n");
    }
    let mut host = started_host(&project).await;
    let started = Instant::now();
    let write = json!({"toolName":"write","input":{"path":"a.py"},"isError":false});
    host.event("tool_result", write, &project).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let started = Instant::now();
    let params = json!({"source":"lsp","paths":["a.py","b.py","c.py"]});
    let request =
        json!({"name":"lens_diagnostics","params":params,"toolCallId":"d","ctx":{"cwd":project.0}});
    let tool = host.request("tool.execute", request).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(
        tool.to_string().contains("Skipped: timed out earlier"),
        "{tool}"
    );
    let fifo = project.0.join("pipe.py");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let started = Instant::now();
    let call = json!({"toolName":"write","input":{"path":"pipe.py"}});
    let reply = host.event("tool_call", call, &project).await;
    assert!(reply.get("result").is_none(), "{reply}");
    assert!(started.elapsed() < Duration::from_secs(1));
    host.finish().await;
}

#[test]
fn menu_rejects_config_values_of_the_wrong_type() {
    let mut value = json!({"languages": 1, "tools": null});
    assert!(crate::tui::slot(&mut value, &["languages", "rust", "enabled"]).is_err());
    *crate::tui::slot(&mut value, &["tools", "ruff", "enabled"]).unwrap() = json!(false);
    assert_eq!(value["tools"]["ruff"]["enabled"], false);
    let mut item = json!(3);
    assert!(crate::tui::slot(&mut item, &["enabled"]).is_err());
}

#[test]
fn findings_cap_keeps_the_count() {
    let items: Vec<Value> = (0..250)
        .map(|i| json!({"message": i.to_string()}))
        .collect();
    let kept = crate::diagnostics::cap(items, Path::new("/a.py"), "x");
    assert_eq!(kept.len(), 201);
    assert_eq!(kept[200]["message"], "50 more findings not shown");
}
