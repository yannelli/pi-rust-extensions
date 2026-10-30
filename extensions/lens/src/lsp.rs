use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{Instant, timeout, timeout_at};

use crate::config::LspServer;
use crate::group::Group;

#[path = "lsp_wire.rs"]
mod wire;

use wire::{command_name, convert, file_uri, language_id, read_message, uri_path, write_message};

const EMPTY_PUBLISH_GRACE: Duration = Duration::from_millis(500);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(300);

/// Opens `path` in a fresh `server` process rooted at `root` and returns the
/// diagnostics the server reports for that file.
pub async fn diagnostics(
    server: &LspServer,
    root: &Path,
    path: &Path,
    timeout_ms: u64,
) -> Result<Vec<Value>, String> {
    let abs_root = std::path::absolute(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let target = std::fs::canonicalize(abs_root.join(path))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let text = tokio::fs::read_to_string(&target)
        .await
        .map_err(|e| format!("{}: {e}", target.display()))?;
    let mut command = Command::new(&server.command);
    command
        .args(&server.args)
        .current_dir(&abs_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to start {}: {e}", server.command))?;
    let mut group = Group::new(child.id());
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        group.kill();
        stop(&mut child).await;
        return Err(format!("{}: missing stdio pipes", server.command));
    };
    let mut session = Session {
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 0,
        root_uri: file_uri(&abs_root),
        target,
    };
    let limit = Duration::from_millis(timeout_ms);
    let result = match timeout(limit, session.run(server, &text)).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "{} timed out after {timeout_ms} ms",
            server.command
        )),
    };
    if result.is_ok() {
        session.shutdown().await;
        if let Ok(Ok(_)) = timeout(SHUTDOWN_GRACE, child.wait()).await {
            group.disarm();
        }
    }
    group.kill();
    stop(&mut child).await;
    let source = command_name(&server.command);
    result.map(|items| {
        items
            .iter()
            .map(|d| convert(d, &session.target, &source))
            .collect()
    })
}

async fn stop(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill().await;
    }
}

enum Event {
    Response(i64, Result<Value, Value>),
    Published(Vec<Value>),
    Other,
}

struct Session {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    root_uri: String,
    target: PathBuf,
}

impl Session {
    async fn run(&mut self, server: &LspServer, text: &str) -> Result<Vec<Value>, String> {
        let init = self.request("initialize", self.initialize_params()).await?;
        let capabilities = self.await_response(init).await?;
        let pull = !capabilities["capabilities"]["diagnosticProvider"].is_null();
        self.notify("initialized", json!({})).await?;
        let uri = file_uri(&self.target);
        let document = json!({
            "uri": uri,
            "languageId": language_id(server, &self.target),
            "version": 1,
            "text": text,
        });
        self.notify("textDocument/didOpen", json!({ "textDocument": document }))
            .await?;
        if pull {
            let params = json!({ "textDocument": { "uri": uri } });
            let id = self.request("textDocument/diagnostic", params).await?;
            if let Ok(report) = self.await_response(id).await
                && report["kind"] == "full"
            {
                return Ok(report["items"].as_array().cloned().unwrap_or_default());
            }
        }
        self.await_published().await
    }

    fn workspace_folders(&self) -> Value {
        let name = self.root_uri.rsplit('/').next().unwrap_or_default();
        json!([{ "uri": self.root_uri, "name": name }])
    }

    fn initialize_params(&self) -> Value {
        json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "pi-lens" },
            "rootUri": self.root_uri,
            "workspaceFolders": self.workspace_folders(),
            "capabilities": {
                "workspace": { "configuration": true, "workspaceFolders": true },
                "textDocument": {
                    "synchronization": { "dynamicRegistration": false },
                    "publishDiagnostics": { "relatedInformation": false },
                    "diagnostic": { "dynamicRegistration": false },
                },
            },
        })
    }

    async fn await_response(&mut self, id: i64) -> Result<Value, String> {
        loop {
            if let Event::Response(got, result) = self.next_event().await?
                && got == id
            {
                return result.map_err(|e| format!("server error: {}", e["message"]));
            }
        }
    }

    /// Returns the first non-empty publish for the target. An empty publish
    /// starts a short grace period for a later non-empty one.
    async fn await_published(&mut self) -> Result<Vec<Value>, String> {
        let mut grace: Option<Instant> = None;
        loop {
            let event = match grace {
                Some(deadline) => match timeout_at(deadline, self.next_event()).await {
                    Ok(event) => event?,
                    Err(_) => return Ok(Vec::new()),
                },
                None => self.next_event().await?,
            };
            if let Event::Published(items) = event {
                if !items.is_empty() {
                    return Ok(items);
                }
                grace.get_or_insert_with(|| Instant::now() + EMPTY_PUBLISH_GRACE);
            }
        }
    }

    async fn next_event(&mut self) -> Result<Event, String> {
        let message = read_message(&mut self.stdout).await?;
        match (message.get("id").cloned(), message["method"].as_str()) {
            (Some(id), Some(method)) => {
                let reply = match self.answer(method, &message["params"]) {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
                };
                write_message(&mut self.stdin, &reply).await?;
                Ok(Event::Other)
            }
            (None, Some("textDocument/publishDiagnostics")) => {
                let params = &message["params"];
                let uri = params["uri"].as_str().and_then(uri_path);
                Ok(match params["diagnostics"].as_array() {
                    Some(items) if uri.as_deref() == Some(self.target.as_path()) => {
                        Event::Published(items.clone())
                    }
                    _ => Event::Other,
                })
            }
            (Some(id), None) => {
                let result = match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                Ok(Event::Response(id.as_i64().unwrap_or(-1), result))
            }
            _ => Ok(Event::Other),
        }
    }

    fn answer(&self, method: &str, params: &Value) -> Result<Value, Value> {
        match method {
            "workspace/configuration" => {
                let count = params["items"].as_array().map_or(0, Vec::len);
                Ok(Value::Array(vec![Value::Null; count]))
            }
            "workspace/workspaceFolders" => Ok(self.workspace_folders()),
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "window/showMessageRequest"
            | "workspace/diagnostic/refresh" => Ok(Value::Null),
            _ => Err(json!({ "code": -32601, "message": format!("unsupported: {method}") })),
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<i64, String> {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        write_message(&mut self.stdin, &message).await?;
        Ok(id)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        write_message(&mut self.stdin, &message).await
    }

    async fn shutdown(&mut self) {
        let polite = async {
            let id = self.request("shutdown", Value::Null).await?;
            self.await_response(id).await?;
            self.notify("exit", Value::Null).await
        };
        let _ = timeout(SHUTDOWN_GRACE, polite).await;
    }
}

#[cfg(test)]
#[path = "lsp_tests.rs"]
mod tests;
