use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::io::BufReader;

use super::diagnostics;
use super::wire::{convert, read_message};
use crate::group::Live;
use crate::tests::Project;

const SERVER: &str = r#"
import json, os, subprocess, sys, time
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

note("pid %d" % os.getpid())
if mode == "garbage":
    while True:
        sys.stdout.buffer.write(b"\xff\xfe garbage " * 4096)
        sys.stdout.buffer.flush()
if mode == "flood":
    while True:
        send({"method": "window/logMessage", "params": {"type": 4, "message": "x" * 4096}})
        send({"method": "textDocument/publishDiagnostics", "params": {"uri": "file:///elsewhere.py", "diagnostics": []}})
while True:
    msg = read()
    method = msg.get("method")
    if method == "initialize":
        send({"id": msg["id"], "result": {"capabilities": {}}})
    elif method == "initialized" and mode == "crash":
        os._exit(7)
    elif method == "textDocument/didOpen":
        uri = msg["params"]["textDocument"]["uri"]
        items = [{"range": {"start": {"line": i, "character": 0}}, "message": "m%d" % i} for i in range(1000)]
        items[0]["range"]["start"] = {"line": 2**64 - 1, "character": 2**64 - 1}
        items[1]["message"] = "y" * 100000
        send({"method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": items}})
    elif method == "shutdown":
        send({"id": msg["id"], "result": None})
    elif method == "exit" and mode == "linger":
        note("child %d" % subprocess.Popen(["sleep", "30"]).pid)
        time.sleep(30)
"#;

struct Fixture {
    project: Project,
    file: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let project = Project::new();
        project.write("server.py", SERVER);
        let file = project.write("a.py", "x = 1\n");
        let log = project.0.join("server.log");
        Self { project, file, log }
    }

    fn server(&self, mode: &str) -> crate::config::LspServer {
        crate::config::LspServer {
            command: "python3".into(),
            args: vec![
                self.project.0.join("server.py").to_string_lossy().into(),
                mode.into(),
                self.log.to_string_lossy().into(),
            ],
            languages: vec!["python".into()],
            enabled: true,
        }
    }

    async fn run(
        &self,
        mode: &str,
        timeout_ms: u64,
    ) -> (Result<Vec<serde_json::Value>, String>, Duration) {
        let started = Instant::now();
        let live = Live::default();
        let result = diagnostics(
            &self.server(mode),
            &self.project.0,
            &self.file,
            timeout_ms,
            &live,
        )
        .await;
        assert_eq!(live.len(), 0, "a process group is still registered");
        (result, started.elapsed())
    }

    fn logged(&self, prefix: &str) -> Vec<String> {
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        log.lines()
            .filter_map(|line| line.strip_prefix(prefix).map(str::to_owned))
            .collect()
    }
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn assert_dead(pids: &[String]) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while pids.iter().any(|pid| alive(pid)) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!pids.iter().any(|pid| alive(pid)), "still alive: {pids:?}");
}

#[tokio::test]
async fn header_line_without_end_is_an_error() {
    let bytes = vec![b'x'; 1024 * 1024];
    let error = read_message(&mut BufReader::new(&bytes[..]))
        .await
        .unwrap_err();
    assert!(error.contains("without an end"), "{error}");
}

#[tokio::test]
async fn oversized_and_malformed_messages_are_errors() {
    let huge = b"Content-Length: 999999999999\r\n\r\n{}";
    let error = read_message(&mut BufReader::new(&huge[..]))
        .await
        .unwrap_err();
    assert!(error.contains("exceeds limit"), "{error}");
    let invalid = b"Content-Length: 4\r\n\xff\xfe: x\r\n\r\n\xff\xfe\xfd\xfc";
    let error = read_message(&mut BufReader::new(&invalid[..]))
        .await
        .unwrap_err();
    assert!(error.contains("invalid message"), "{error}");
    let missing = b"\r\n{}";
    let error = read_message(&mut BufReader::new(&missing[..]))
        .await
        .unwrap_err();
    assert!(error.contains("Content-Length"), "{error}");
}

#[test]
fn positions_at_u64_max_do_not_overflow() {
    let item = json!({"range":{"start":{"line":u64::MAX,"character":u64::MAX}},"message":"m"});
    let converted = convert(&item, Path::new("/a.py"), "fake");
    assert_eq!(converted["line"], u64::MAX);
    assert_eq!(converted["column"], u64::MAX);
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_round_trip() {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/home/u/caf\xe9 dir/a.py"));
    let uri = super::wire::file_uri(path);
    assert_eq!(uri, "file:///home/u/caf%E9%20dir/a.py");
    assert_eq!(super::wire::uri_path(&uri).as_deref(), Some(path));
}

#[tokio::test]
async fn flooding_server_times_out_and_is_killed() {
    let fx = Fixture::new();
    let (result, elapsed) = fx.run("flood", 500).await;
    assert!(result.unwrap_err().contains("timed out"));
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert_dead(&fx.logged("pid ")).await;
}

#[tokio::test]
async fn garbage_output_fails_fast() {
    let fx = Fixture::new();
    let (result, elapsed) = fx.run("garbage", 10_000).await;
    assert!(result.unwrap_err().contains("without an end"));
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert_dead(&fx.logged("pid ")).await;
}

#[tokio::test]
async fn crash_after_initialize_is_an_error() {
    let fx = Fixture::new();
    let (result, elapsed) = fx.run("crash", 10_000).await;
    assert!(result.unwrap_err().contains("closed"));
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

#[tokio::test]
async fn many_diagnostics_are_capped_and_clipped() {
    let fx = Fixture::new();
    let (result, _) = fx.run("many", 10_000).await;
    let found = result.unwrap();
    assert_eq!(found.len(), crate::diagnostics::MAX_FINDINGS + 1);
    assert_eq!(found[0]["line"], u64::MAX);
    assert!(found[1]["message"].as_str().unwrap().len() <= crate::diagnostics::MAX_MESSAGE_BYTES);
    assert_eq!(found[200]["message"], "800 more findings not shown");
}

#[cfg(unix)]
#[tokio::test]
async fn server_that_ignores_exit_is_killed_with_its_children() {
    let fx = Fixture::new();
    let (result, elapsed) = fx.run("linger", 10_000).await;
    assert_eq!(result.unwrap().len(), crate::diagnostics::MAX_FINDINGS + 1);
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    let mut pids = fx.logged("pid ");
    pids.extend(fx.logged("child "));
    assert_dead(&pids).await;
}

#[cfg(unix)]
#[tokio::test]
async fn fifo_target_is_skipped_without_reading() {
    let fx = Fixture::new();
    let fifo = fx.project.0.join("pipe.py");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let started = Instant::now();
    let live = Live::default();
    let error = diagnostics(&fx.server("many"), &fx.project.0, &fifo, 10_000, &live)
        .await
        .unwrap_err();
    assert!(error.contains("not a regular file"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(1));
}
