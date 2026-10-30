use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::Instant;

use crate::config::{Config, ToolCommand};
use crate::group::Group;
use crate::{detection, diagnostics, lsp};

pub async fn run(config: &Config, cwd: &Path, path: &Path) -> Vec<Value> {
    if !config.enabled {
        return vec![];
    }
    let Some(language) = detection::language(path) else {
        return vec![];
    };
    if !config.language_enabled(language) {
        return vec![];
    }
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
    let deadline = Instant::now() + Duration::from_millis(config.pipeline_timeout_ms);
    let mut findings = vec![];
    for tool in tools {
        let Some(limit) = budget(deadline, config.timeout_ms) else {
            findings.push(skipped(path, &tool.name, config.pipeline_timeout_ms));
            continue;
        };
        match execute(&tool, &root, path, limit, config.trusted).await {
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
                findings.extend(parsed);
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
        let Some(limit) = budget(deadline, config.timeout_ms) else {
            findings.push(skipped(path, &server.command, config.pipeline_timeout_ms));
            continue;
        };
        let limit_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX).max(1);
        let call = lsp::diagnostics(server, &root, path, limit_ms);
        let result = tokio::time::timeout(limit, call)
            .await
            .unwrap_or_else(|_| Err(format!("{} timed out after {limit_ms} ms", server.command)));
        match result {
            Ok(items) => findings.extend(items.into_iter().map(|mut item| {
                item["kind"] = serde_json::json!("lsp");
                item
            })),
            Err(error) => findings.push(diagnostics::finding(
                path,
                &server.command,
                "warning",
                &error,
            )),
        }
    }
    findings
}

fn budget(deadline: Instant, timeout_ms: u64) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .map(|left| left.min(Duration::from_millis(timeout_ms)))
}

fn skipped(path: &Path, source: &str, total_ms: u64) -> Value {
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
        let keep = count.min((1024 * 1024usize).saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
    }
    Ok(bytes)
}

async fn execute(
    tool: &ToolCommand,
    root: &Path,
    path: &Path,
    limit: Duration,
    trusted: bool,
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
    let mut group = Group::new(child.id());
    let stdout = child.stdout.take().ok_or("Missing stdout")?;
    let stderr = child.stderr.take().ok_or("Missing stderr")?;
    let work = async {
        let (status, stdout, stderr) =
            tokio::try_join!(child.wait(), capture(stdout), capture(stderr))?;
        Ok::<_, std::io::Error>((
            status.success(),
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            ),
        ))
    };
    match tokio::time::timeout(limit, work).await {
        Ok(result) => {
            group.disarm();
            result.map_err(|error| error.to_string())
        }
        Err(_) => Err(format!(
            "{} timed out after {} ms",
            tool.name,
            limit.as_millis()
        )),
    }
}
