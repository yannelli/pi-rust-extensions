#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use serde_json::json;

use super::Project;
#[cfg(unix)]
use super::executable;
#[cfg(unix)]
use super::protocol::Host;
use crate::detection;
#[cfg(unix)]
use crate::{config, pipeline};

#[cfg(unix)]
fn write_result(path: &str) -> serde_json::Value {
    json!({"toolName":"write","input":{"path":path},"isError":false})
}

#[cfg(unix)]
async fn start(host: &mut Host, project: &Project) {
    host.request("initialize", json!({"protocolVersion":1,"cwd":project.0,"agentDir":project.0.join("agent"),"hasUI":host.has_ui})).await;
    host.event("session_start", json!({}), project).await;
}

#[cfg(unix)]
fn marker_command(marker: &Path) -> serde_json::Value {
    let script = format!("echo \"$0\" >> '{}'", marker.display());
    json!({"name":"mark","language":"python","stage":"lint","command":"/bin/sh","args":["-c",script,"{path}"]})
}

#[cfg(unix)]
#[tokio::test]
async fn untrusted_project_runs_no_detected_tools_or_local_binaries() {
    for trusted in [false, true] {
        let project = Project::new();
        project.write("Cargo.toml", "[package]\nname = \"a\"\n");
        project.write("src/main.rs", "fn main() {}\n");
        let marker = project.0.join("ran");
        for name in ["cargo", "rustfmt"] {
            let script = format!("#!/bin/sh\necho {name} >> '{}'\n", marker.display());
            executable(&project, &format!("node_modules/.bin/{name}"), &script);
        }
        let mut host = Host::new(crate::extension());
        host.trusted = trusted;
        host.has_ui = true;
        start(&mut host, &project).await;
        for _ in 0..2 {
            host.event("tool_result", write_result("src/main.rs"), &project)
                .await;
        }
        let notices = host
            .calls
            .iter()
            .filter(|call| call["method"] == "ui.notify")
            .count();
        assert_eq!(notices, usize::from(!trusted), "{:?}", host.calls);
        assert_eq!(marker.exists(), trusted, "trusted={trusted}");
        host.finish().await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn writes_outside_cwd_run_no_tools() {
    let project = Project::new();
    let outside = Project::new();
    let target = outside.write("b.py", "x = 1\n");
    std::os::unix::fs::symlink(&target, project.0.join("link.py")).unwrap();
    let marker = project.0.join("ran");
    let config = json!({"autoDetect":false,"commands":[marker_command(&marker)]});
    project.write("agent/ext/lens.json", &config.to_string());
    let mut host = Host::new(crate::extension());
    start(&mut host, &project).await;
    let name = outside.0.file_name().unwrap().to_string_lossy();
    for path in [
        "link.py".to_owned(),
        target.to_string_lossy().into_owned(),
        format!("../{name}/b.py"),
    ] {
        host.event("tool_result", write_result(&path), &project)
            .await;
    }
    assert!(!marker.exists());
    project.write("a.py", "y = 2\n");
    host.event("tool_result", write_result("a.py"), &project)
        .await;
    let ran = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(ran.lines().count(), 1, "{ran}");
    assert!(ran.trim_end().ends_with("a.py"), "{ran}");
    host.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn tool_result_releases_state_while_the_pipeline_runs() {
    let project = Project::new();
    let config = json!({"autoDetect":false,"commands":[{"name":"slow","language":"python","stage":"lint","command":"/bin/sh","args":["-c","sleep 2"]}]});
    project.write("agent/ext/lens.json", &config.to_string());
    project.write("a.py", "x = 1\n");
    let mut host = Host::new(crate::extension());
    start(&mut host, &project).await;
    let params = host.event_params("tool_result", write_result("a.py"), &project);
    host.send("slow", "event", params).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    let call = json!({"toolName":"write","input":{"path":"new.py"}});
    host.event("tool_call", call, &project).await;
    assert!(started.elapsed() < Duration::from_secs(1));
    host.wait("slow").await;
    host.finish().await;
}

#[cfg(unix)]
fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_grandchildren_and_the_pipeline_has_a_total_cap() {
    let project = Project::new();
    let file = project.write("a.py", "");
    let pids = project.0.join("pids");
    let tool = |name: &str| config::ToolCommand {
        name: name.to_owned(),
        language: "python".to_owned(),
        stage: config::Stage::Lint,
        command: "/bin/sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "sleep 30 & echo $! >> \"$0\"; wait".to_owned(),
            pids.to_string_lossy().into_owned(),
        ],
        enabled: true,
    };
    let config = config::Config {
        auto_detect: false,
        timeout_ms: 300,
        pipeline_timeout_ms: 300,
        commands: vec![tool("first"), tool("second")],
        ..Default::default()
    };
    let started = Instant::now();
    let findings = pipeline::run(&config, &project.0, &file).await;
    assert!(started.elapsed() < Duration::from_secs(2), "{findings:?}");
    let messages: Vec<_> = findings
        .iter()
        .map(|item| item["message"].as_str().unwrap().to_owned())
        .collect();
    assert!(messages[0].contains("first timed out"), "{messages:?}");
    assert!(messages[1].starts_with("Skipped"), "{messages:?}");
    let pids = std::fs::read_to_string(pids).unwrap();
    assert_eq!(pids.lines().count(), 1, "{pids}");
    let deadline = Instant::now() + Duration::from_secs(3);
    while pids.lines().any(alive) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !pids.lines().any(alive),
        "grandchildren still alive: {pids}"
    );
}

#[test]
fn rustfmt_edition_follows_the_nearest_manifest() {
    let project = Project::new();
    project.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"*\"]\n\n[workspace.package]\nedition = \"2024\" # current\n",
    );
    project.write(
        "a/Cargo.toml",
        "[package]\nname = \"a\"\nedition.workspace = true\n",
    );
    project.write(
        "b/Cargo.toml",
        "[package]\nname = \"b\"\nedition = { workspace = true }\n",
    );
    project.write(
        "c/Cargo.toml",
        "[package]\nname = \"c\"\n\n[dependencies]\nedition = \"2015\"\n",
    );
    project.write(
        "d/Cargo.toml",
        "[workspace.package]\nedition = \"2015\"\n\n[package]\nname = \"d\"\nedition = '2018'\n",
    );
    project.write(
        "e/Cargo.toml",
        "[package]\nname = \"e\"\nedition = \"2018\"\n",
    );
    project.write("e/f/Cargo.toml", "[package]\nname = \"f\"\n");
    let edition = |dir: &str| detection::rust_edition(&project.0.join(dir));
    assert_eq!(edition("a"), "2024");
    assert_eq!(edition("b"), "2024");
    assert_eq!(edition("c"), "2021");
    assert_eq!(edition("d"), "2018");
    assert_eq!(edition("e/f"), "2021");
    assert_eq!(edition(""), "2024");
    let args = &detection::detect(&project.0.join("a"), "rust")[0].args;
    assert_eq!(args, &["--edition", "2024", "{path}"]);
}
