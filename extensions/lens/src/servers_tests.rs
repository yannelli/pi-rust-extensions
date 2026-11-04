use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::Project;
#[cfg(unix)]
use super::protocol::Host;
use crate::config;
#[cfg(unix)]
use crate::pipeline::{self, Runner, Scan};

/// Logs what it receives, asks for one setting, and publishes one finding.
#[cfg(unix)]
const SERVER: &str = r#"
import json, os, sys
log = sys.argv[1]

def note(text):
    with open(log, "a") as f:
        f.write(text + "\n")

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        name, value = line.decode().split(":", 1)
        if name.lower() == "content-length":
            length = int(value)
    return json.loads(sys.stdin.buffer.read(length))

def send(message):
    message["jsonrpc"] = "2.0"
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()

note("env %s" % os.environ.get("FAKE_ENV", ""))
note("cwd %s" % os.getcwd())
if len(sys.argv) > 2 and sys.argv[2] == "hang":
    while True:
        read()
while True:
    msg = read()
    method = msg.get("method")
    if method == "initialize":
        note("init %s" % json.dumps(msg["params"].get("initializationOptions")))
        note("root %s" % msg["params"]["rootUri"])
        send({"id": msg["id"], "result": {"capabilities": {}}})
    elif method == "workspace/didChangeConfiguration":
        note("settings %s" % json.dumps(msg["params"]["settings"]))
    elif method == "textDocument/didOpen":
        doc = msg["params"]["textDocument"]
        note("language %s" % doc["languageId"])
        send({"id": "cfg", "method": "workspace/configuration",
              "params": {"items": [{"section": "fake.level"}, {"section": "fake.none"}]}})
        note("config %s" % json.dumps(read().get("result")))
        diag = {"range": {"start": {"line": 0, "character": 0}}, "severity": 1, "message": "custom-finding"}
        send({"method": "textDocument/publishDiagnostics", "params": {"uri": doc["uri"], "diagnostics": [diag]}})
    elif method == "shutdown":
        send({"id": msg["id"], "result": None})
    elif method == "exit":
        sys.exit(0)
"#;

#[cfg(unix)]
fn server_entry(project: &Project, mode: &str) -> Value {
    let script = project.write("server.py", SERVER);
    let log = project.0.join("server.log");
    json!({
        "plugin": "zz@fixtures",
        "command": "python3",
        "args": [script, log, mode],
        "extensionToLanguage": { ".zz": "zzlang" },
        "env": { "FAKE_ENV": "from-env" },
        "initializationOptions": { "mode": "strict" },
        "settings": { "fake": { "level": 3 } },
        "workspaceFolder": "work",
    })
}

#[cfg(unix)]
fn log(project: &Project) -> String {
    std::fs::read_to_string(project.0.join("server.log")).unwrap_or_default()
}

#[test]
fn unknown_fields_warn_and_keep_the_config_valid() {
    let project = Project::new();
    let agent = project.0.join("agent");
    let config = json!({
        "colour": "blue",
        "lsp": { "servers": [{ "command": "gopls", "languages": ["go"], "restartOnCrash": true, "wat": 1, "transport": "tcp" }], "extra": 1 },
        "commands": [{ "name": "c", "language": "go", "stage": "lint", "command": "x", "note": "" }]
    });
    project.write("agent/ext/lens.json", &config.to_string());
    let (config, _) = config::load(&agent, &project.0, true).unwrap();
    assert_eq!(config.lsp.servers[0].command, "gopls");
    let text = config.warnings.join("\n");
    for needle in [
        "\"colour\"",
        "\"lsp.extra\"",
        "\"commands.c.note\"",
        "gopls: unknown field \"wat\"",
        "restartOnCrash and maxRestarts are not applied",
        "transport \"tcp\" is not supported",
    ] {
        assert!(text.contains(needle), "{needle} missing in {text}");
    }
    let bad = json!({ "lsp": { "servers": [{ "command": "gopls" }] } });
    project.write("agent/ext/lens.json", &bad.to_string());
    let error = config::load(&agent, &project.0, true).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("languages or extensionToLanguage")
    );
}

#[test]
fn server_files_merge_by_name_and_need_trust_for_the_project() {
    let project = Project::new();
    let agent = project.0.join("agent");
    let user = json!({ "servers": {
        "shared": { "command": "user-ls", "extensionToLanguage": { ".zz": "zz" } },
        "broken": { "command": "", "languages": ["go"] },
        "typed": { "command": 3 },
    }, "other": true });
    project.write("agent/ext/lens-lsp.json", &user.to_string());
    let local =
        json!({ "servers": { "shared": { "command": "project-ls", "languages": ["go"] } } });
    project.write(".pi/ext/lens-lsp.json", &local.to_string());
    let (config, ignored) = config::load(&agent, &project.0, false).unwrap();
    assert!(ignored);
    let names: Vec<_> = config
        .lsp
        .servers
        .iter()
        .map(|s| s.command.as_str())
        .collect();
    assert_eq!(names, ["user-ls"]);
    assert_eq!(config.lsp.servers[0].name.as_deref(), Some("shared"));
    let text = config.warnings.join("\n");
    assert!(text.contains("broken skipped: it needs command"), "{text}");
    assert!(text.contains("typed skipped"), "{text}");
    assert!(text.contains("unknown field \"other\""), "{text}");
    let (config, ignored) = config::load(&agent, &project.0, true).unwrap();
    assert!(!ignored);
    let names: Vec<_> = config
        .lsp
        .servers
        .iter()
        .map(|s| s.command.as_str())
        .collect();
    assert_eq!(names, ["project-ls"]);
    assert_eq!(
        config.file_language(std::path::Path::new("a.zz")),
        None,
        "the project entry replaced the .zz mapping"
    );
    project.write("agent/ext/lens-lsp.json", "{ not json");
    let (config, _) = config::load(&agent, &project.0, false).unwrap();
    assert!(config.warnings[0].contains("is not valid JSON"));
}

#[cfg(unix)]
#[tokio::test]
async fn custom_server_checks_a_new_extension_with_its_settings() {
    let project = Project::new();
    std::fs::create_dir_all(project.0.join("work")).unwrap();
    let file = project.write("src/a.zz", "zz code\n");
    let entry = server_entry(&project, "serve");
    let servers = json!({ "servers": { "fake": entry } });
    project.write("agent/ext/lens-lsp.json", &servers.to_string());
    let (config, _) = config::load(&project.0.join("agent"), &project.0, true).unwrap();
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.file_language(&file).as_deref(), Some("zzlang"));
    let findings = pipeline::run(&config, &project.0, &file, &Runner::default()).await;
    assert_eq!(findings.len(), 1, "{findings:?}\n{}", log(&project));
    assert_eq!(findings[0]["message"], "custom-finding");
    assert_eq!(findings[0]["kind"], "lsp");
    let log = log(&project);
    let work = project.0.join("work").canonicalize().unwrap();
    for needle in [
        "env from-env".to_owned(),
        format!("cwd {}", work.display()),
        "init {\"mode\": \"strict\"}".to_owned(),
        "settings {\"fake\": {\"level\": 3}}".to_owned(),
        "language zzlang".to_owned(),
        "config [3, null]".to_owned(),
    ] {
        assert!(log.contains(&needle), "{needle} missing in {log}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn diagnostics_false_skips_writes_but_not_requests() {
    let project = Project::new();
    std::fs::create_dir_all(project.0.join("work")).unwrap();
    let file = project.write("a.zz", "zz\n");
    let mut entry = server_entry(&project, "serve");
    entry["diagnostics"] = json!(false);
    project.write(
        "agent/ext/lens-lsp.json",
        &json!({ "servers": { "fake": entry } }).to_string(),
    );
    let (config, _) = config::load(&project.0.join("agent"), &project.0, true).unwrap();
    let runner = Runner::default();
    assert!(
        pipeline::run(&config, &project.0, &file, &runner)
            .await
            .is_empty()
    );
    let scan = Scan {
        config: &config,
        cwd: &project.0,
        root: &project.0,
        path: &file,
        language: "zzlang",
        deadline: tokio::time::Instant::now() + Duration::from_secs(10),
    };
    let findings = scan.servers(&runner, false).await;
    assert_eq!(findings[0]["message"], "custom-finding", "{findings:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn startup_timeout_limits_the_initialize_wait() {
    let project = Project::new();
    let file = project.write("a.zz", "zz\n");
    let mut entry = server_entry(&project, "hang");
    entry["startupTimeout"] = json!(200);
    entry["shutdownTimeout"] = json!(100);
    entry.as_object_mut().unwrap().remove("workspaceFolder");
    project.write(
        "agent/ext/lens-lsp.json",
        &json!({ "servers": { "fake": entry } }).to_string(),
    );
    let (config, _) = config::load(&project.0.join("agent"), &project.0, true).unwrap();
    let started = Instant::now();
    let findings = pipeline::run(&config, &project.0, &file, &Runner::default()).await;
    assert!(started.elapsed() < Duration::from_secs(3));
    let message = findings[0]["message"].as_str().unwrap();
    assert!(
        message.contains("fake did not answer within 200 ms"),
        "{message}"
    );
}

#[cfg(unix)]
async fn start(host: &mut Host, project: &Project) {
    let init = json!({"protocolVersion":1,"cwd":project.0,"agentDir":project.0.join("agent"),"hasUI":host.has_ui});
    host.request("initialize", init).await;
    host.event("session_start", json!({}), project).await;
}

#[cfg(unix)]
async fn write_zz(host: &mut Host, project: &Project) -> Value {
    let event = json!({"toolName":"write","input":{"path":"a.zz"},"isError":false});
    host.event("tool_result", event, project).await;
    let call =
        json!({"name":"lens_diagnostics","params":{},"toolCallId":"d","ctx":{"cwd":project.0}});
    host.request("tool.execute", call).await
}

#[cfg(unix)]
#[tokio::test]
async fn servers_added_after_session_start_apply_on_the_next_write() {
    let project = Project::new();
    std::fs::create_dir_all(project.0.join("work")).unwrap();
    project.write("a.zz", "zz\n");
    let mut host = Host::new(crate::extension());
    start(&mut host, &project).await;
    let result = write_zz(&mut host, &project).await;
    assert_eq!(result["details"]["total"], 0, "{result}");
    let entry = server_entry(&project, "serve");
    project.write(
        "agent/ext/lens-lsp.json",
        &json!({ "servers": { "fake": entry } }).to_string(),
    );
    let result = write_zz(&mut host, &project).await;
    assert!(result.to_string().contains("custom-finding"), "{result}");
    std::fs::remove_file(project.0.join("agent/ext/lens-lsp.json")).unwrap();
    let result = write_zz(&mut host, &project).await;
    assert_eq!(result["details"]["total"], 0, "{result}");
    host.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn untrusted_project_server_file_runs_nothing_and_warns_once() {
    for trusted in [false, true] {
        let project = Project::new();
        std::fs::create_dir_all(project.0.join("work")).unwrap();
        project.write("a.zz", "zz\n");
        let entry = server_entry(&project, "serve");
        project.write(
            ".pi/ext/lens-lsp.json",
            &json!({ "servers": { "fake": entry } }).to_string(),
        );
        project.write("agent/ext/lens.json", r#"{"autoDetect":false}"#);
        let mut host = Host::new(crate::extension());
        host.trusted = trusted;
        host.has_ui = true;
        start(&mut host, &project).await;
        let result = write_zz(&mut host, &project).await;
        write_zz(&mut host, &project).await;
        assert_eq!(result.to_string().contains("custom-finding"), trusted);
        assert_eq!(log(&project).is_empty(), !trusted);
        let notices = host
            .calls
            .iter()
            .filter(|call| call["method"] == "ui.notify")
            .count();
        assert_eq!(notices, usize::from(!trusted), "{:?}", host.calls);
        host.finish().await;
    }
}
