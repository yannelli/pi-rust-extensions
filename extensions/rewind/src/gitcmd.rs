use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use pi_ext_sdk::{Error, Result, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const SCRUBBED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_COMMON_DIR",
];

pub fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::msg(format!("path is not UTF-8: {}", path.display())))
}

pub async fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let output = git_output(cwd, args).await?;
    if !output.status.success() {
        bail!("{}", failure(args, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn failure(args: &[&str], output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("git {} failed: {}", args.join(" "), stderr.trim())
}

/// Runs git and returns its output whatever the exit status.
pub async fn git_output(cwd: Option<&Path>, args: &[&str]) -> Result<Output> {
    git_output_within(cwd, args, None, GIT_TIMEOUT)
        .await?
        .ok_or_else(|| Error::msg(format!("git {} timed out", args.join(" "))))
}

/// Runs git with `input` on stdin; `None` when it runs longer than `limit`, which kills it.
pub async fn git_output_within(
    cwd: Option<&Path>,
    args: &[&str],
    input: Option<&[u8]>,
    limit: Duration,
) -> Result<Option<Output>> {
    let mut command = Command::new("git");
    command
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    for key in SCRUBBED_ENV {
        command.env_remove(key);
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let run = async {
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let write = async move {
            if let (Some(input), Some(mut stdin)) = (input, stdin) {
                stdin.write_all(input).await?;
            }
            Ok::<_, std::io::Error>(())
        };
        let (written, output) = tokio::join!(write, child.wait_with_output());
        written?;
        output
    };
    match tokio::time::timeout(limit, run).await {
        Ok(output) => Ok(Some(output?)),
        Err(_) => Ok(None),
    }
}
