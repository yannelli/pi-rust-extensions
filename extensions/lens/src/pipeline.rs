use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout, timeout_at};

use crate::config::{Config, ToolCommand};
use crate::group::{Group, Live};
use crate::{detection, diagnostics, lsp};

pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const PIPE_GRACE: Duration = Duration::from_millis(500);

/// Shared by all scans of one session: one scan runs at a time, and an LSP server
/// that stalled (see [`full_wait`]) is skipped until the next session start or `/lens` change.
#[derive(Clone)]
pub struct Runner {
    gate: Arc<Semaphore>,
    stalled: Arc<Mutex<HashSet<String>>>,
    pub live: Live,
}

impl Default for Runner {
    fn default() -> Self {
        Self {
            gate: Arc::new(Semaphore::new(1)),
            stalled: Arc::default(),
            live: Live::default(),
        }
    }
}

impl Runner {
    fn stalled(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.stalled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn reset(&self) {
        self.stalled().clear();
    }

    /// Returns a skip finding when `source` timed out before.
    pub fn skip(&self, path: &Path, source: &str) -> Option<Value> {
        self.stalled().contains(source).then(|| {
            let message = "Skipped: timed out earlier in this session. Run /lens or start a new session to retry";
            diagnostics::finding(path, source, "warning", message)
        })
    }

    pub fn record(&self, source: &str, error: &str) {
        if error.contains("timed out") {
            self.stalled().insert(source.to_owned());
        }
    }
}

pub async fn run(config: &Config, cwd: &Path, path: &Path, runner: &Runner) -> Vec<Value> {
    if !config.enabled {
        return vec![];
    }
    let Some(language) = detection::language(path) else {
        return vec![];
    };
    if !config.language_enabled(language) {
        return vec![];
    }
    let deadline = Instant::now() + Duration::from_millis(config.pipeline_timeout_ms);
    let Ok(Ok(_permit)) = timeout_at(deadline, runner.gate.acquire()).await else {
        return vec![skipped(path, "lens", config.pipeline_timeout_ms)];
    };
    let root = detection::root(path, cwd, language);
    let mut tools = if config.auto_detect && config.trusted {
        detection::detect(&root, language)
    } else {
        vec![]
    };
    for command in config
        .commands
        .iter()
        .filter(|item| item.language == language)
    {
        tools.retain(|tool| tool.name != command.name);
        tools.push(command.clone());
    }
    tools.retain(|tool| tool.enabled && config.tool_enabled(&tool.name));
    tools.sort_by_key(|tool| tool.stage);
    let mut findings = vec![];
    for tool in tools {
        let Some(limit) = budget(deadline, config.timeout_ms) else {
            findings.push(skipped(path, &tool.name, config.pipeline_timeout_ms));
            continue;
        };
        match execute(&tool, &root, path, limit, config.trusted, &runner.live).await {
            Ok((success, output)) => {
                let parsed = diagnostics::parse(&tool.name, &output, &root, path);
                if !success && parsed.is_empty() {
                    let message = if output.trim().is_empty() {
                        "Command failed without output"
                    } else {
                        output.trim()
                    };
                    findings.push(diagnostics::finding(path, &tool.name, "error", message));
                }
                findings.extend(diagnostics::cap(parsed, path, &tool.name));
            }
            Err(error) => findings.push(diagnostics::finding(path, &tool.name, "warning", &error)),
        }
    }
    for server in config
        .lsp
        .servers
        .iter()
        .filter(|server| server.enabled && server.languages.iter().any(|item| item == language))
    {
        if let Some(skip) = runner.skip(path, &server.command) {
            findings.push(skip);
            continue;
        }
        let Some(limit) = budget(deadline, config.timeout_ms) else {
            findings.push(skipped(path, &server.command, config.pipeline_timeout_ms));
            continue;
        };
        match lsp_diagnostics(server, &root, path, limit, &runner.live).await {
            Ok(items) => findings.extend(items),
            Err(error) => {
                if full_wait(config, limit) {
                    runner.record(&server.command, &error);
                }
                findings.push(diagnostics::finding(
                    path,
                    &server.command,
                    "warning",
                    &error,
                ));
            }
        }
    }
    findings
}

/// `lsp::diagnostics` under a hard `limit`, with findings marked as LSP findings.
pub async fn lsp_diagnostics(
    server: &crate::config::LspServer,
    root: &Path,
    path: &Path,
    limit: Duration,
    live: &Live,
) -> Result<Vec<Value>, String> {
    let limit_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX).max(1);
    let call = lsp::diagnostics(server, root, path, limit_ms, live);
    let items = timeout(
        limit + lsp::SHUTDOWN_GRACE + Duration::from_millis(200),
        call,
    )
    .await
    .unwrap_or_else(|_| Err(format!("{} timed out after {limit_ms} ms", server.command)))?;
    Ok(items
        .into_iter()
        .map(|mut item| {
            item["kind"] = serde_json::json!("lsp");
            item
        })
        .collect())
}

/// True when `limit` is close to the most a server can get, so a timeout means
/// the server stalled rather than a short remaining budget.
pub fn full_wait(config: &Config, limit: Duration) -> bool {
    let most = config.timeout_ms.min(config.pipeline_timeout_ms);
    limit >= Duration::from_millis(most / 10 * 9)
}

pub fn budget(deadline: Instant, timeout_ms: u64) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .map(|left| left.min(Duration::from_millis(timeout_ms)))
}

pub fn skipped(path: &Path, source: &str, total_ms: u64) -> Value {
    let message = format!("Skipped: the Lens pipeline used its {total_ms} ms limit");
    diagnostics::finding(path, source, "warning", &message)
}

async fn capture(mut reader: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut bytes = vec![];
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let keep = count.min(MAX_OUTPUT_BYTES.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
    }
    Ok(bytes)
}

type Readers = JoinSet<(usize, std::io::Result<Vec<u8>>)>;

async fn drain(readers: &mut Readers, output: &mut [Vec<u8>; 2]) {
    while let Some(joined) = readers.join_next().await {
        if let Ok((index, Ok(bytes))) = joined {
            output[index] = bytes;
        }
    }
}

async fn execute(
    tool: &ToolCommand,
    root: &Path,
    path: &Path,
    limit: Duration,
    trusted: bool,
    live: &Live,
) -> Result<(bool, String), String> {
    let args: Vec<String> = tool
        .args
        .iter()
        .map(|arg| {
            arg.replace("{path}", &path.to_string_lossy())
                .replace("{root}", &root.to_string_lossy())
        })
        .collect();
    let mut command = Command::new(&tool.command);
    command
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    if trusted {
        let mut paths = vec![root.join("node_modules/.bin")];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        command.env(
            "PATH",
            std::env::join_paths(paths).map_err(|error| error.to_string())?,
        );
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("{}: {error}", tool.command))?;
    let mut group = Group::new(child.id(), live);
    let stdout = child.stdout.take().ok_or("Missing stdout")?;
    let stderr = child.stderr.take().ok_or("Missing stderr")?;
    let mut readers = Readers::new();
    readers.spawn(async move { (0, capture(stdout).await) });
    readers.spawn(async move { (1, capture(stderr).await) });
    let deadline = Instant::now() + limit;
    let status = match timeout_at(deadline, child.wait()).await {
        Ok(status) => status.map_err(|error| error.to_string())?,
        Err(_) => {
            group.stop().await;
            return Err(format!(
                "{} timed out after {} ms",
                tool.name,
                limit.as_millis()
            ));
        }
    };
    let mut output = [Vec::new(), Vec::new()];
    let grace = deadline.min(Instant::now() + PIPE_GRACE);
    if timeout_at(grace, drain(&mut readers, &mut output))
        .await
        .is_ok()
    {
        group.disarm();
    } else {
        // A process left behind still holds the pipes, so the group still exists.
        group.stop().await;
        let _ = timeout(PIPE_GRACE, drain(&mut readers, &mut output)).await;
    }
    Ok((
        status.success(),
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&output[0]),
            String::from_utf8_lossy(&output[1])
        ),
    ))
}
