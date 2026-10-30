//! The `command` status line: runs the user's command with session JSON on stdin and shows
//! what it prints, like Claude Code.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::config::CommandConfig;

pub const DEBOUNCE: Duration = Duration::from_millis(300);
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_LINES: usize = 8;

fn shell() -> &'static str {
    static SHELL: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    SHELL.get_or_init(|| {
        let bash = std::process::Command::new("bash")
            .args(["-c", "true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if bash.is_ok_and(|status| status.success()) {
            "bash"
        } else {
            "sh"
        }
    })
}

/// Runs `command` in bash with `payload` on stdin; `None` when it fails, exits non-zero, or times out.
pub async fn run_command(
    config: &CommandConfig,
    payload: &str,
    columns: usize,
    cwd: &Path,
) -> Option<Vec<String>> {
    let mut process = tokio::process::Command::new(shell());
    process.arg("-c").arg(&config.command);
    if cwd.is_dir() {
        process.current_dir(cwd);
    }
    process
        .env("COLUMNS", columns.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = process.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = format!("{payload}\n");
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let output = tokio::time::timeout(TIMEOUT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output_lines(&String::from_utf8_lossy(&output.stdout)))
}

/// Output lines without trailing blank lines, tabs as spaces, and at most [`MAX_LINES`].
pub fn output_lines(stdout: &str) -> Vec<String> {
    let mut lines: Vec<String> = stdout
        .lines()
        .take(MAX_LINES)
        .map(|line| line.trim_end_matches('\r').replace('\t', "    "))
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// The output fitted to `width` with the left padding.
pub fn fit_output(output: &[String], padding: usize, width: usize) -> Vec<String> {
    let padding = padding.min(width);
    let pad = " ".repeat(padding);
    output
        .iter()
        .map(|line| {
            format!(
                "{pad}{}",
                pi_ext_sdk::style::truncate(line, width - padding)
            )
        })
        .collect()
}
