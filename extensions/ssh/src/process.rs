use std::process::Stdio;
use std::time::Duration;

use pi_ext_sdk::prelude::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 1024 * 1024;

pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
    pub truncated: bool,
}

impl Output {
    pub fn result(self) -> ToolResult {
        let text = if self.stderr.is_empty() {
            self.stdout.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        };
        let text = if self.truncated {
            format!("{text}\n[Output truncated at 1 MiB per stream]")
        } else {
            text
        };
        let result = ToolResult::text(text)
            .details(json!({"exitCode": self.code, "truncated": self.truncated}));
        if self.code == Some(0) {
            result
        } else {
            result.failed()
        }
    }
}

async fn collect(
    mut reader: impl AsyncRead + Unpin,
    ctx: Option<ToolCtx>,
) -> Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(output.len());
        let take = count.min(remaining);
        output.extend_from_slice(&chunk[..take]);
        truncated |= take < count;
        if let Some(ctx) = &ctx {
            if take > 0 {
                ctx.update(String::from_utf8_lossy(&chunk[..take]).into_owned())?;
            }
        }
    }
    Ok((output, truncated))
}

pub async fn run(
    command: Command,
    input: Option<String>,
    seconds: u64,
    cancel: CancellationToken,
    ctx: Option<ToolCtx>,
) -> Result<Output> {
    run_inner(command, input, seconds, cancel, ctx, false).await
}

pub async fn run_remote(
    command: Command,
    input: Option<String>,
    seconds: u64,
    cancel: CancellationToken,
    ctx: Option<ToolCtx>,
) -> Result<Output> {
    let input = input.unwrap_or_default();
    let framed = format!("{}\n{input}", input.len());
    run_inner(command, Some(framed), seconds, cancel, ctx, true).await
}

async fn run_inner(
    mut command: Command,
    input: Option<String>,
    seconds: u64,
    cancel: CancellationToken,
    ctx: Option<ToolCtx>,
    keep_open: bool,
) -> Result<Output> {
    if seconds == 0 {
        return Err(Error::msg("timeout must be greater than zero"));
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::msg("SSH stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::msg("SSH stderr unavailable"))?;
    let mut stdin = child.stdin.take();
    let communicate = async {
        let write = async {
            if let (Some(writer), Some(input)) = (stdin.as_mut(), input) {
                writer.write_all(input.as_bytes()).await?;
            }
            if !keep_open {
                drop(stdin.take());
            }
            Ok::<_, Error>(())
        };
        let (_, (stdout, out_cut), (stderr, err_cut)) =
            tokio::try_join!(write, collect(stdout, ctx.clone()), collect(stderr, ctx))?;
        Ok::<_, Error>((stdout, stderr, out_cut || err_cut))
    };
    let output = tokio::select! {
        _ = cancel.cancelled() => Err(Error::msg("SSH command cancelled")),
        result = tokio::time::timeout(Duration::from_secs(seconds), async {
            let data = communicate.await?;
            let status = child.wait().await?;
            Ok::<_, Error>((data, status))
        }) => match result {
            Ok(result) => result,
            Err(_) => Err(Error::msg(format!("SSH command timed out after {seconds}s"))),
        }
    };
    drop(stdin);
    match output {
        Ok(((stdout, stderr, truncated), status)) => Ok(Output {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            code: status.code(),
            truncated,
        }),
        Err(error) => {
            if !keep_open
                || tokio::time::timeout(Duration::from_secs(1), child.wait())
                    .await
                    .is_err()
            {
                child.kill().await?;
                child.wait().await?;
            }
            Err(error)
        }
    }
}
