use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::config::{Config, ToolCommand};
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
    let mut tools = if config.auto_detect {
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
        match execute(&tool, &root, path, config.timeout_ms).await {
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
        match lsp::diagnostics(server, &root, path, config.timeout_ms).await {
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
    timeout_ms: u64,
) -> Result<(bool, String), String> {
    let args: Vec<String> = tool
        .args
        .iter()
        .map(|arg| {
            arg.replace("{path}", &path.to_string_lossy())
                .replace("{root}", &root.to_string_lossy())
        })
        .collect();
    let mut paths = vec![root.join("node_modules/.bin")];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    let mut child = Command::new(&tool.command)
        .args(args)
        .current_dir(root)
        .env(
            "PATH",
            std::env::join_paths(paths).map_err(|error| error.to_string())?,
        )
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("{}: {error}", tool.command))?;
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
    match tokio::time::timeout(Duration::from_millis(timeout_ms), work).await {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            Err(format!("{} timed out after {timeout_ms} ms", tool.name))
        }
    }
}
