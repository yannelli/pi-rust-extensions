use std::path::{Path, PathBuf};
use std::time::Instant;

use super::diagnostics;
use super::wire::{file_uri, language_id, uri_path};
use crate::config::LspServer;

const FAKE_SERVER: &str = r#"
import json, os, re, sys
mode, log = sys.argv[1], sys.argv[2]

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

def publish(uri, items):
    send({"method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": items}})

DIAGS = [
    {"range": {"start": {"line": 2, "character": 4}}, "severity": 1, "message": "bad", "source": "fake"},
    {"range": {"start": {"line": 0, "character": 0}}, "severity": 2, "message": "meh"},
    {"range": {"start": {"line": 1, "character": 1}}, "severity": 4, "message": "tip"},
]
note("pid %d" % os.getpid())
if mode == "exit":
    sys.exit(3)
if mode == "spawn":
    import subprocess
    note("child %d" % subprocess.Popen(["sleep", "30"]).pid)
while True:
    msg = read()
    method = msg.get("method")
    note("method %s" % method)
    if mode in ("hang", "spawn"):
        continue
    if method == "initialize":
        caps = {"diagnosticProvider": {"interFileDependencies": False}} if mode == "pull" else {}
        send({"id": msg["id"], "result": {"capabilities": caps}})
    elif method == "textDocument/didOpen":
        uri = msg["params"]["textDocument"]["uri"]
        note("uri %s" % uri)
        if mode == "publish":
            send({"id": "cfg", "method": "workspace/configuration", "params": {"items": [{}, {}]}})
            note("config %s" % json.dumps(read().get("result")))
            send({"id": 99, "method": "custom/unknown", "params": {}})
            note("unknown %s" % json.dumps(read().get("error")))
            publish("file:///elsewhere/other.py", [DIAGS[0]])
            publish(re.sub(r"%[0-9A-F]{2}", lambda m: m.group(0).lower(), uri), DIAGS)
        elif mode == "empty":
            publish(uri, [])
    elif method == "textDocument/diagnostic":
        send({"id": msg["id"], "result": {"kind": "full", "items": DIAGS[:1]}})
    elif method == "shutdown":
        send({"id": msg["id"], "result": None})
    elif method == "exit":
        sys.exit(0)
"#;

struct Fixture {
    dir: PathBuf,
    file: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("lens-lsp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        std::fs::write(dir.join("server.py"), FAKE_SERVER).unwrap();
        let file = dir.join("my file é.py");
        std::fs::write(&file, "x = 1\n").unwrap();
        let log = dir.join("server.log");
        Self { dir, file, log }
    }

    fn server(&self, mode: &str) -> LspServer {
        let script = self.dir.join("server.py");
        LspServer {
            command: "python3".into(),
            args: vec![
                script.to_string_lossy().into_owned(),
                mode.into(),
                self.log.to_string_lossy().into_owned(),
            ],
            languages: vec!["python".into()],
            enabled: true,
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn pid(&self) -> String {
        let log = self.log();
        let line = log
            .lines()
            .find_map(|l| l.strip_prefix("pid "))
            .expect("pid line");
        line.to_string()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[tokio::test]
async fn publish_returns_only_target_diagnostics() {
    let fx = Fixture::new("publish");
    let found = diagnostics(
        &fx.server("publish"),
        &fx.dir,
        Path::new("my file é.py"),
        10_000,
    )
    .await
    .unwrap();
    let path = fx.file.to_string_lossy().into_owned();
    assert_eq!(found.len(), 3);
    assert_eq!(found[0]["path"], path);
    assert_eq!(
        (found[0]["line"].as_u64(), found[0]["column"].as_u64()),
        (Some(3), Some(5))
    );
    assert_eq!(found[0]["severity"], "error");
    assert_eq!(found[0]["message"], "bad");
    assert_eq!(found[0]["source"], "fake");
    assert_eq!(
        (found[1]["severity"].as_str(), found[1]["source"].as_str()),
        (Some("warning"), Some("python3"))
    );
    assert_eq!(found[2]["severity"], "hint");

    let log = fx.log();
    assert!(
        log.contains(&format!("uri {}", file_uri(&fx.file))),
        "{log}"
    );
    assert!(log.contains("config [null, null]"), "{log}");
    assert!(log.contains("-32601"), "{log}");
    assert!(
        log.contains("method shutdown") && log.contains("method exit"),
        "{log}"
    );
    assert!(!alive(&fx.pid()));
}

#[tokio::test]
async fn pull_uses_document_diagnostic_request() {
    let fx = Fixture::new("pull");
    let found = diagnostics(&fx.server("pull"), &fx.dir, &fx.file, 10_000)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["message"], "bad");
    assert!(fx.log().contains("method textDocument/diagnostic"));
}

#[tokio::test]
async fn empty_publish_returns_no_diagnostics() {
    let fx = Fixture::new("empty");
    let found = diagnostics(&fx.server("empty"), &fx.dir, &fx.file, 10_000)
        .await
        .unwrap();
    assert!(found.is_empty());
}

#[tokio::test]
async fn timeout_kills_and_reaps_server() {
    let fx = Fixture::new("hang");
    let started = Instant::now();
    let error = diagnostics(&fx.server("hang"), &fx.dir, &fx.file, 300)
        .await
        .unwrap_err();
    assert!(error.contains("timed out"), "{error}");
    assert!(started.elapsed().as_secs() < 3);
    assert!(!alive(&fx.pid()));
}

async fn assert_child_stops(fx: &Fixture) {
    let log = fx.log();
    let child = log
        .lines()
        .find_map(|l| l.strip_prefix("child "))
        .expect("child line");
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    while alive(child) && Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!alive(child), "server child {child} still alive");
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_and_drop_kill_server_descendants() {
    let fx = Fixture::new("spawn-timeout");
    let error = diagnostics(&fx.server("spawn"), &fx.dir, &fx.file, 300)
        .await
        .unwrap_err();
    assert!(error.contains("timed out"), "{error}");
    assert_child_stops(&fx).await;
    let fx = Fixture::new("spawn-drop");
    let server = fx.server("spawn");
    let call = diagnostics(&server, &fx.dir, &fx.file, 10_000);
    let limit = std::time::Duration::from_millis(500);
    assert!(tokio::time::timeout(limit, call).await.is_err());
    assert_child_stops(&fx).await;
}

#[tokio::test]
async fn early_exit_is_an_error() {
    let fx = Fixture::new("exit");
    let error = diagnostics(&fx.server("exit"), &fx.dir, &fx.file, 10_000)
        .await
        .unwrap_err();
    assert!(error.contains("closed"), "{error}");
}

#[tokio::test]
async fn missing_command_is_an_error() {
    let fx = Fixture::new("missing");
    let mut server = fx.server("publish");
    server.command = "/nonexistent/lens-lsp".into();
    let error = diagnostics(&server, &fx.dir, &fx.file, 1_000)
        .await
        .unwrap_err();
    assert!(error.starts_with("failed to start"), "{error}");
}

#[test]
fn file_uri_escapes_and_round_trips() {
    let path = Path::new("/tmp/a b/é#1.rs");
    let uri = file_uri(path);
    assert_eq!(uri, "file:///tmp/a%20b/%C3%A9%231.rs");
    assert_eq!(uri_path(&uri).as_deref(), Some(path));
    assert_eq!(
        uri_path("file://localhost/tmp/a%20b/%c3%a9%231.rs").as_deref(),
        Some(path)
    );
    assert_eq!(uri_path("untitled:foo"), None);
}

#[test]
fn language_id_prefers_known_extension() {
    let mut server = Fixture::new("language").server("publish");
    server.languages = vec!["javascript".into()];
    assert_eq!(language_id(&server, Path::new("a.ts")), "typescript");
    assert_eq!(language_id(&server, Path::new("a.tsx")), "typescriptreact");
    assert_eq!(language_id(&server, Path::new("a.jsx")), "javascriptreact");
    assert_eq!(language_id(&server, Path::new("a.vue")), "javascript");
    server.languages.clear();
    assert_eq!(language_id(&server, Path::new("a.vue")), "vue");
}
